use std::sync::Arc;

use rusqlite::{Connection, params, params_from_iter};

use crate::{
    adapter::json_scalar_to_string,
    adapter::sql::adapter::SqlError,
    adapter::sql::runtime::SqlRuntimeBuilder,
    adapter::{
        AdapterError, AdapterResult, GuardState, SkipReason, StorageAdapter, WriteDecision, decide,
    },
    event::Event,
    routing::StorageKind,
    runtime::config::MigrationPolicy,
    upcast::UpcasterRegistry,
};

/// SQLite runtime adapter (Phase 4: idempotent; Phase 12: sequence-gated
/// upsert; Phase 13: sequence-gated delete & tombstones)
pub struct SqliteAdapter {
    id: String,
    priority: u32,
    path: String,
    builder: SqlRuntimeBuilder,
    upcasters: Arc<UpcasterRegistry>,
    migration_policy: MigrationPolicy,
}

impl SqliteAdapter {
    pub fn new(
        id: String,
        path: String,
        priority: u32,
        builder: SqlRuntimeBuilder,
        upcasters: Arc<UpcasterRegistry>,
        migration_policy: MigrationPolicy,
    ) -> Self {
        Self {
            id,
            path,
            priority,
            builder,
            upcasters,
            migration_policy,
        }
    }

    fn failure(&self, error: AdapterError) -> AdapterResult {
        AdapterResult::failure(self.id.clone(), StorageKind::Sql, error)
    }

    /// Read one guard lane — `(target_table, entity_key, facet)` (Phase 15.2).
    /// The default facet is stored as `''`, so a pre-Phase-15 row (written
    /// before the `facet` column existed on a freshly created table) reads
    /// back as the default lane unchanged.
    fn read_guard_lane(
        tx: &rusqlite::Transaction<'_>,
        table: &str,
        entity_key: &str,
        facet: &str,
    ) -> Result<Option<GuardState>, rusqlite::Error> {
        match tx.query_row(
            "SELECT last_sequence, deleted, permanent FROM conduit_projection_state
             WHERE target_table = ?1 AND entity_key = ?2 AND facet = ?3",
            params![table, entity_key, facet],
            |row| {
                Ok(GuardState {
                    last_sequence: row.get::<_, i64>(0)? as u64,
                    deleted: row.get::<_, i64>(1)? != 0,
                    permanent: row.get::<_, i64>(2)? != 0,
                })
            },
        ) {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

impl StorageAdapter for SqliteAdapter {
    fn kind(&self) -> StorageKind {
        StorageKind::Sql
    }

    fn id(&self) -> &str {
        &self.id
    }

    fn priority(&self) -> u32 {
        self.priority
    }

    fn handle(&self, event: &Event) -> AdapterResult {
        // Build SQL projection (upcasting the payload to the mapping's target version first)
        let projection = match self.builder.build(event, &self.upcasters) {
            Ok(p) => p,
            Err(SqlError::UnsupportedVersion {
                from_version,
                to_version,
                reason,
                ..
            }) => {
                return match self.migration_policy {
                    MigrationPolicy::Strict => AdapterResult::unsupported_version(
                        self.id.clone(),
                        StorageKind::Sql,
                        reason,
                        from_version,
                        to_version,
                    ),
                    MigrationPolicy::IgnoreUnmatched => AdapterResult::skipped_versioned(
                        self.id.clone(),
                        StorageKind::Sql,
                        SkipReason::UnsupportedVersion,
                        from_version,
                        to_version,
                    ),
                };
            }
            Err(e) => return self.failure(AdapterError::WriteFailed(e.to_string())),
        };
        let (
            sql,
            values,
            source_version,
            projected_version,
            entity_key,
            key_values,
            primary_key_columns,
            table,
            on_existing,
            operation,
            permanent,
            facet,
        ) = (
            projection.sql,
            projection.values,
            projection.source_version,
            projection.projected_version,
            projection.entity_key,
            projection.key_values,
            projection.primary_key_columns,
            projection.table,
            projection.on_existing,
            projection.operation,
            projection.permanent,
            projection.facet,
        );
        let is_named_facet = !facet.is_empty();

        let mut conn = match Connection::open(&self.path) {
            Ok(c) => c,
            Err(e) => return self.failure(AdapterError::WriteFailed(e.to_string())),
        };

        // PHASE 11.2/13.2/15.2: ensure the entity-aware idempotency guard table
        // exists. Supersedes the Phase 4 `conduit_events` (event_id-only)
        // guard: two different event_ids that resolve to the same (table,
        // entity_key) are now both caught, not just a replayed event_id.
        // `deleted`/`permanent` (Phase 13.2/13.4): the guard row is never
        // removed once written — it becomes a tombstone instead.
        // `facet` (Phase 15.2): the primary key is `(target_table, entity_key,
        // facet)`, default facet stored as `''`. A guard table that predates
        // Phase 15 (2-column PK, no `facet` column) must be dropped — it is a
        // derived cache and replay rebuilds it; there is no in-place migration
        // because the ON CONFLICT target below needs the 3-column constraint.
        if let Err(e) = conn.execute(
            "CREATE TABLE IF NOT EXISTS conduit_projection_state (
                target_table TEXT NOT NULL,
                entity_key TEXT NOT NULL,
                facet TEXT NOT NULL DEFAULT '',
                last_sequence INTEGER NOT NULL,
                last_event_id TEXT NOT NULL,
                deleted INTEGER NOT NULL DEFAULT 0,
                permanent INTEGER NOT NULL DEFAULT 0,
                processed_at TEXT NOT NULL,
                PRIMARY KEY (target_table, entity_key, facet)
            )",
            [],
        ) {
            return self.failure(AdapterError::WriteFailed(e.to_string()));
        }

        let tx = match conn.transaction() {
            Ok(tx) => tx,
            Err(e) => return self.failure(AdapterError::WriteFailed(e.to_string())),
        };

        // PHASE 15.3: entity-existence pre-check for a named facet. A facet
        // update can only touch a row that the default facet already created
        // and has not tombstoned — inserting a sparse row would violate
        // `NOT NULL` on the create mapping's columns, and only a default-facet
        // `upsert` resurrects a deleted entity.
        if is_named_facet {
            let default_state = match Self::read_guard_lane(&tx, &table, &entity_key, "") {
                Ok(v) => v,
                Err(e) => return self.failure(AdapterError::WriteFailed(e.to_string())),
            };
            let entity_present = matches!(default_state, Some(s) if !s.deleted);
            if !entity_present {
                return AdapterResult::skipped_versioned(
                    self.id.clone(),
                    StorageKind::Sql,
                    SkipReason::EntityAbsent,
                    source_version,
                    projected_version,
                );
            }
        }

        // PHASE 12.2/13.1/15.2: read this facet's lane — read-decide-write, all
        // inside this transaction. SQLite serializes writers, so this is atomic
        // against concurrent dispatch.
        let stored: Option<GuardState> =
            match Self::read_guard_lane(&tx, &table, &entity_key, &facet) {
                Ok(v) => v,
                Err(e) => return self.failure(AdapterError::WriteFailed(e.to_string())),
            };

        // A named facet is always a sequence-gated partial replace of its own
        // columns; `decide()` itself is unchanged (Phase 15.2) — the adapter
        // just passes `Replace` for the facet lane. The default facet keeps the
        // mapping's declared `on_existing`.
        let effective_mode = if is_named_facet {
            crate::adapter::OnExisting::Replace
        } else {
            on_existing
        };

        // The gated decision (Phase 12.2, extended 13.1) — before any real
        // write, so a duplicate, stale, or tombstoned event never reaches the
        // INSERT/UPSERT/DELETE statement.
        let decision = decide(operation, effective_mode, stored, event.sequence);
        let was_tombstoned_default = !is_named_facet && matches!(stored, Some(s) if s.deleted);

        match decision {
            WriteDecision::SkipIdempotent => {
                return AdapterResult::skipped_versioned(
                    self.id.clone(),
                    StorageKind::Sql,
                    SkipReason::AlreadyProjected,
                    source_version,
                    projected_version,
                );
            }
            WriteDecision::SkipStale => {
                return AdapterResult::skipped_versioned(
                    self.id.clone(),
                    StorageKind::Sql,
                    SkipReason::StaleSequence,
                    source_version,
                    projected_version,
                );
            }
            WriteDecision::SkipTombstoned => {
                return AdapterResult::skipped_versioned(
                    self.id.clone(),
                    StorageKind::Sql,
                    SkipReason::Tombstoned,
                    source_version,
                    projected_version,
                );
            }
            WriteDecision::SkipAlreadyDeleted => {
                // Phase 13.2: still bump last_sequence so a later, truly
                // out-of-order resurrection attempt compares against the
                // highest delete sequence seen, not a stale one.
                if let Err(e) = tx.execute(
                    "UPDATE conduit_projection_state
                        SET last_sequence = ?4, last_event_id = ?5, processed_at = datetime('now')
                     WHERE target_table = ?1 AND entity_key = ?2 AND facet = ?3",
                    params![
                        table,
                        entity_key,
                        facet,
                        event.sequence as i64,
                        event.event_id
                    ],
                ) {
                    return self.failure(AdapterError::WriteFailed(e.to_string()));
                }
                if let Err(e) = tx.commit() {
                    return self.failure(AdapterError::WriteFailed(e.to_string()));
                }
                return AdapterResult::skipped_versioned(
                    self.id.clone(),
                    StorageKind::Sql,
                    SkipReason::AlreadyDeleted,
                    source_version,
                    projected_version,
                );
            }
            WriteDecision::Insert | WriteDecision::Update | WriteDecision::Delete => {}
        }

        // Execute the projection write.
        if decision == WriteDecision::Delete {
            let where_clause = primary_key_columns
                .iter()
                .map(|c| format!("{c} = ?"))
                .collect::<Vec<_>>()
                .join(" AND ");
            let delete_sql = format!("DELETE FROM {table} WHERE {where_clause}");
            let delete_params: Vec<String> = key_values.iter().map(json_scalar_to_string).collect();
            if let Err(e) = tx.execute(&delete_sql, params_from_iter(delete_params.iter())) {
                return self.failure(AdapterError::WriteFailed(e.to_string()));
            }
        } else {
            // Plain INSERT for `ignore`; the statement doubles as an upsert for
            // `replace` and as a partial-column upsert for a named facet — see
            // `SqlMapping::build`.
            let bind_params: Vec<String> = values.iter().map(json_scalar_to_string).collect();
            if let Err(e) = tx.execute(&sql, params_from_iter(bind_params.iter())) {
                return self.failure(AdapterError::WriteFailed(e.to_string()));
            }
        }

        // PHASE 11.2/12.3/13.2/15.2: record this facet lane's new state AFTER
        // the write, same transaction, via one upsert statement.
        let (deleted, row_permanent) = if decision == WriteDecision::Delete {
            (true, permanent)
        } else {
            (false, false)
        };
        if let Err(e) = tx.execute(
            "INSERT INTO conduit_projection_state
                (target_table, entity_key, facet, last_sequence, last_event_id, deleted, permanent, processed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, datetime('now'))
             ON CONFLICT (target_table, entity_key, facet) DO UPDATE SET
                last_sequence = excluded.last_sequence,
                last_event_id = excluded.last_event_id,
                deleted = excluded.deleted,
                permanent = excluded.permanent,
                processed_at = excluded.processed_at",
            params![
                table,
                entity_key,
                facet,
                event.sequence as i64,
                event.event_id,
                deleted as i64,
                row_permanent as i64
            ],
        ) {
            return self.failure(AdapterError::WriteFailed(e.to_string()));
        }

        // PHASE 15.3: a default-facet delete cascades its tombstone to every
        // facet lane; a default-facet resurrection clears every facet lane's
        // tombstone. Both keep the whole entity's lanes consistent in one
        // transaction.
        if !is_named_facet && decision == WriteDecision::Delete {
            if let Err(e) = tx.execute(
                "UPDATE conduit_projection_state
                    SET deleted = 1, permanent = ?4, last_sequence = ?3,
                        last_event_id = ?5, processed_at = datetime('now')
                 WHERE target_table = ?1 AND entity_key = ?2 AND facet <> ''",
                params![
                    table,
                    entity_key,
                    event.sequence as i64,
                    permanent as i64,
                    event.event_id
                ],
            ) {
                return self.failure(AdapterError::WriteFailed(e.to_string()));
            }
        } else if was_tombstoned_default
            && decision == WriteDecision::Insert
            && let Err(e) = tx.execute(
                "UPDATE conduit_projection_state
                    SET deleted = 0, permanent = 0
                 WHERE target_table = ?1 AND entity_key = ?2 AND facet <> ''",
                params![table, entity_key],
            )
        {
            return self.failure(AdapterError::WriteFailed(e.to_string()));
        }

        if let Err(e) = tx.commit() {
            return self.failure(AdapterError::WriteFailed(e.to_string()));
        }

        match decision {
            WriteDecision::Insert => AdapterResult::created_versioned(
                self.id.clone(),
                StorageKind::Sql,
                source_version,
                projected_version,
            ),
            WriteDecision::Update => AdapterResult::updated_versioned(
                self.id.clone(),
                StorageKind::Sql,
                source_version,
                projected_version,
            ),
            WriteDecision::Delete => AdapterResult::deleted_versioned(
                self.id.clone(),
                StorageKind::Sql,
                source_version,
                projected_version,
            ),
            // Every skip variant returned early above.
            WriteDecision::SkipIdempotent
            | WriteDecision::SkipStale
            | WriteDecision::SkipAlreadyDeleted
            | WriteDecision::SkipTombstoned => self.failure(AdapterError::WriteFailed(
                "unreachable: skip decision reached the write path".to_string(),
            )),
        }
    }
}
