use std::sync::Arc;

use rusqlite::{Connection, params, params_from_iter};

use crate::{
    adapter::json_scalar_to_string,
    adapter::sql::adapter::SqlError,
    adapter::sql::runtime::SqlRuntimeBuilder,
    adapter::{AdapterError, AdapterResult, SkipReason, StorageAdapter, WriteDecision, decide},
    event::Event,
    routing::StorageKind,
    runtime::config::MigrationPolicy,
    upcast::UpcasterRegistry,
};

/// SQLite runtime adapter (Phase 4: idempotent; Phase 12: sequence-gated upsert)
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
        let (sql, values, source_version, projected_version, entity_key, table, on_existing) = (
            projection.sql,
            projection.values,
            projection.source_version,
            projection.projected_version,
            projection.entity_key,
            projection.table,
            projection.on_existing,
        );

        let mut conn = match Connection::open(&self.path) {
            Ok(c) => c,
            Err(e) => return self.failure(AdapterError::WriteFailed(e.to_string())),
        };

        // PHASE 11.2: ensure the entity-aware idempotency guard table exists.
        // Supersedes the Phase 4 `conduit_events` (event_id-only) guard: two
        // different event_ids that resolve to the same (table, entity_key) are
        // now both caught, not just a replayed event_id. `entity_key` is the
        // Phase 11.1 canonical encoding — an ordered JSON array — so a
        // single-column key can never collide with a differently-split
        // composite one. `last_sequence` (Phase 11.2: stored, not compared) is
        // read and compared as of Phase 12.2/12.3.
        if let Err(e) = conn.execute(
            "CREATE TABLE IF NOT EXISTS conduit_projection_state (
                target_table TEXT NOT NULL,
                entity_key TEXT NOT NULL,
                last_sequence INTEGER NOT NULL,
                last_event_id TEXT NOT NULL,
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

        // PHASE 12.2/12.3: read the guard's current state for this entity —
        // read-decide-write, all inside this transaction. SQLite serializes
        // writers, so this is atomic against concurrent dispatch.
        let stored_last_sequence: Option<u64> = match tx.query_row(
            "SELECT last_sequence FROM conduit_projection_state WHERE target_table = ?1 AND entity_key = ?2",
            params![table, entity_key],
            |row| row.get::<_, i64>(0),
        ) {
            Ok(v) => Some(v as u64),
            Err(rusqlite::Error::QueryReturnedNoRows) => None,
            Err(e) => return self.failure(AdapterError::WriteFailed(e.to_string())),
        };
        let exists = stored_last_sequence.is_some();

        // The gated decision (Phase 12.2) — before the real write, so a
        // duplicate or stale event never reaches the INSERT/UPSERT statement.
        let is_update = match decide(on_existing, exists, stored_last_sequence, event.sequence) {
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
            WriteDecision::Insert => false,
            WriteDecision::Update => true,
        };

        // Convert JSON values to bind params
        let params: Vec<String> = values.iter().map(json_scalar_to_string).collect();

        // Execute the projection (plain INSERT for `ignore`; the statement
        // doubles as an upsert for `replace` — see `SqlMapping::build`).
        if let Err(e) = tx.execute(&sql, params_from_iter(params.iter())) {
            return self.failure(AdapterError::WriteFailed(e.to_string()));
        }

        // PHASE 11.2/12.3: record the guard row's new state AFTER the write,
        // same transaction — an INSERT for a brand-new entity, an UPDATE to
        // bump `last_sequence` for one being replaced.
        let guard_result = if is_update {
            tx.execute(
                "UPDATE conduit_projection_state
                    SET last_sequence = ?3, last_event_id = ?4, processed_at = datetime('now')
                 WHERE target_table = ?1 AND entity_key = ?2",
                params![table, entity_key, event.sequence as i64, event.event_id],
            )
        } else {
            tx.execute(
                "INSERT INTO conduit_projection_state
                    (target_table, entity_key, last_sequence, last_event_id, processed_at)
                 VALUES (?1, ?2, ?3, ?4, datetime('now'))",
                params![table, entity_key, event.sequence as i64, event.event_id],
            )
        };
        if let Err(e) = guard_result {
            return self.failure(AdapterError::WriteFailed(e.to_string()));
        }

        if let Err(e) = tx.commit() {
            return self.failure(AdapterError::WriteFailed(e.to_string()));
        }

        if is_update {
            AdapterResult::updated_versioned(
                self.id.clone(),
                StorageKind::Sql,
                source_version,
                projected_version,
            )
        } else {
            AdapterResult::created_versioned(
                self.id.clone(),
                StorageKind::Sql,
                source_version,
                projected_version,
            )
        }
    }
}
