use std::sync::Arc;

use rusqlite::{Connection, params, params_from_iter};

use crate::{
    adapter::json_scalar_to_string,
    adapter::sql::adapter::SqlError,
    adapter::sql::runtime::SqlRuntimeBuilder,
    adapter::{AdapterError, AdapterResult, StorageAdapter},
    event::Event,
    routing::StorageKind,
    runtime::config::MigrationPolicy,
    upcast::UpcasterRegistry,
};

/// SQLite runtime adapter (Phase 4: idempotent)
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
                    MigrationPolicy::IgnoreUnmatched => AdapterResult::skipped_version(
                        self.id.clone(),
                        StorageKind::Sql,
                        reason,
                        from_version,
                        to_version,
                    ),
                };
            }
            Err(e) => return self.failure(AdapterError::WriteFailed(e.to_string())),
        };
        let (sql, values, source_version, projected_version, entity_key, table) = (
            projection.sql,
            projection.values,
            projection.source_version,
            projection.projected_version,
            projection.entity_key,
            projection.table,
        );

        let mut conn = match Connection::open(&self.path) {
            Ok(c) => c,
            Err(e) => return self.failure(AdapterError::WriteFailed(e.to_string())),
        };

        // PHASE 11.2: ensure entity-aware idempotency guard table exists.
        // Supersedes the Phase 4 `conduit_events` (event_id-only) guard: two
        // different event_ids that resolve to the same (table, entity_key) are
        // now both caught, not just a replayed event_id. `entity_key` is the
        // Phase 11.1 canonical encoding — an ordered JSON array — so a
        // single-column key can never collide with a differently-split
        // composite one.
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

        // PHASE 11.2: entity-aware idempotency check, BEFORE the real INSERT —
        // a second creation event for an already-created entity is skipped
        // cleanly here; the INSERT (and any PK constraint) is never reached.
        let already_processed: bool = match tx.query_row(
            "SELECT 1 FROM conduit_projection_state WHERE target_table = ?1 AND entity_key = ?2",
            params![table, entity_key],
            |_| Ok(()),
        ) {
            Ok(_) => true,
            Err(rusqlite::Error::QueryReturnedNoRows) => false,
            Err(e) => return self.failure(AdapterError::WriteFailed(e.to_string())),
        };

        if already_processed {
            return AdapterResult::skipped_version(
                self.id.clone(),
                StorageKind::Sql,
                format!("entity {} already projected into '{}'", entity_key, table),
                source_version,
                projected_version,
            );
        }

        // Convert JSON values to bind params
        let params: Vec<String> = values.iter().map(json_scalar_to_string).collect();

        // Execute projection
        if let Err(e) = tx.execute(&sql, params_from_iter(params.iter())) {
            return self.failure(AdapterError::WriteFailed(e.to_string()));
        }

        // PHASE 11.2: record the guard row AFTER the write, same transaction.
        // `last_sequence` is stored, not compared — first creation wins (no
        // Upsert/CAS semantics exist yet; see Phase 11 non-goals).
        if let Err(e) = tx.execute(
            "INSERT INTO conduit_projection_state
                (target_table, entity_key, last_sequence, last_event_id, processed_at)
             VALUES (?1, ?2, ?3, ?4, datetime('now'))",
            params![table, entity_key, event.sequence as i64, event.event_id],
        ) {
            return self.failure(AdapterError::WriteFailed(e.to_string()));
        }

        if let Err(e) = tx.commit() {
            return self.failure(AdapterError::WriteFailed(e.to_string()));
        }

        AdapterResult::success_versioned(
            self.id.clone(),
            StorageKind::Sql,
            source_version,
            projected_version,
        )
    }
}
