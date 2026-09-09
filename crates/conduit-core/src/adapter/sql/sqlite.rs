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
        );

        let mut conn = match Connection::open(&self.path) {
            Ok(c) => c,
            Err(e) => return self.failure(AdapterError::WriteFailed(e.to_string())),
        };

        // PHASE 11.2/13.2: ensure the entity-aware idempotency guard table
        // exists. Supersedes the Phase 4 `conduit_events` (event_id-only)
        // guard: two different event_ids that resolve to the same (table,
        // entity_key) are now both caught, not just a replayed event_id.
        // `deleted`/`permanent` (Phase 13.2/13.4): the guard row is never
        // removed once written — it becomes a tombstone instead, so a later
        // stale event (a replayed older create, a re-delivered delete) is
        // still gated.
        if let Err(e) = conn.execute(
            "CREATE TABLE IF NOT EXISTS conduit_projection_state (
                target_table TEXT NOT NULL,
                entity_key TEXT NOT NULL,
                last_sequence INTEGER NOT NULL,
                last_event_id TEXT NOT NULL,
                deleted INTEGER NOT NULL DEFAULT 0,
                permanent INTEGER NOT NULL DEFAULT 0,
                processed_at TEXT NOT NULL,
                PRIMARY KEY (target_table, entity_key)
            )",
            [],
        ) {
            return self.failure(AdapterError::WriteFailed(e.to_string()));
        }

        let tx = match conn.transaction() {
            Ok(tx) => tx,
            Err(e) => return self.failure(AdapterError::WriteFailed(e.to_string())),
        };

        // PHASE 12.2/13.1: read the guard's current state for this entity —
        // read-decide-write, all inside this transaction. SQLite serializes
        // writers, so this is atomic against concurrent dispatch.
        let stored: Option<GuardState> = match tx.query_row(
            "SELECT last_sequence, deleted, permanent FROM conduit_projection_state
             WHERE target_table = ?1 AND entity_key = ?2",
            params![table, entity_key],
            |row| {
                Ok(GuardState {
                    last_sequence: row.get::<_, i64>(0)? as u64,
                    deleted: row.get::<_, i64>(1)? != 0,
                    permanent: row.get::<_, i64>(2)? != 0,
                })
            },
        ) {
            Ok(v) => Some(v),
            Err(rusqlite::Error::QueryReturnedNoRows) => None,
            Err(e) => return self.failure(AdapterError::WriteFailed(e.to_string())),
        };

        // The gated decision (Phase 12.2, extended 13.1) — before any real
        // write, so a duplicate, stale, or tombstoned event never reaches the
        // INSERT/UPSERT/DELETE statement.
        let decision = decide(operation, on_existing, stored, event.sequence);

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
                        SET last_sequence = ?3, last_event_id = ?4, processed_at = datetime('now')
                     WHERE target_table = ?1 AND entity_key = ?2",
                    params![table, entity_key, event.sequence as i64, event.event_id],
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
            // Plain INSERT for `ignore`; the statement doubles as an upsert
            // for `replace` — see `SqlMapping::build`.
            let bind_params: Vec<String> = values.iter().map(json_scalar_to_string).collect();
            if let Err(e) = tx.execute(&sql, params_from_iter(bind_params.iter())) {
                return self.failure(AdapterError::WriteFailed(e.to_string()));
            }
        }

        // PHASE 11.2/12.3/13.2: record the guard row's new state AFTER the
        // write, same transaction, via one upsert statement — it doesn't
        // matter whether a guard row already existed (fresh entity,
        // resurrection, or ordinary update all land here the same way).
        let (deleted, row_permanent) = if decision == WriteDecision::Delete {
            (true, permanent)
        } else {
            (false, false)
        };
        if let Err(e) = tx.execute(
            "INSERT INTO conduit_projection_state
                (target_table, entity_key, last_sequence, last_event_id, deleted, permanent, processed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, datetime('now'))
             ON CONFLICT (target_table, entity_key) DO UPDATE SET
                last_sequence = excluded.last_sequence,
                last_event_id = excluded.last_event_id,
                deleted = excluded.deleted,
                permanent = excluded.permanent,
                processed_at = excluded.processed_at",
            params![
                table,
                entity_key,
                event.sequence as i64,
                event.event_id,
                deleted as i64,
                row_permanent as i64
            ],
        ) {
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
