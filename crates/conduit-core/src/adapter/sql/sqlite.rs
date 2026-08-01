use std::sync::Arc;

use rusqlite::{params, params_from_iter, Connection};
use serde_json::Value;

use crate::{
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
        let (sql, values, source_version, projected_version) = (
            projection.sql,
            projection.values,
            projection.source_version,
            projection.projected_version,
        );

        let mut conn = match Connection::open(&self.path) {
            Ok(c) => c,
            Err(e) => return self.failure(AdapterError::WriteFailed(e.to_string())),
        };

        // PHASE 4: ensure idempotency table exists
        if let Err(e) = conn.execute(
            "CREATE TABLE IF NOT EXISTS conduit_events (
                event_id TEXT PRIMARY KEY,
                processed_at TEXT NOT NULL
            )",
            [],
        ) {
            return self.failure(AdapterError::WriteFailed(e.to_string()));
        }

        let tx = match conn.transaction() {
            Ok(tx) => tx,
            Err(e) => return self.failure(AdapterError::WriteFailed(e.to_string())),
        };

        // PHASE 4: idempotency check
        let already_processed: bool = match tx.query_row(
            "SELECT 1 FROM conduit_events WHERE event_id = ?",
            params![event.event_id],
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
                "event already processed".to_string(),
                source_version,
                projected_version,
            );
        }

        // Convert JSON values
        let params: Vec<String> = values
            .iter()
            .map(|v| match v {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.to_string(),
                Value::Bool(b) => b.to_string(),
                Value::Null => String::new(),
                other => other.to_string(),
            })
            .collect();

        // Execute projection
        if let Err(e) = tx.execute(&sql, params_from_iter(params.iter())) {
            return self.failure(AdapterError::WriteFailed(e.to_string()));
        }

        // PHASE 4: record event as processed
        if let Err(e) = tx.execute(
            "INSERT INTO conduit_events (event_id, processed_at)
             VALUES (?, datetime('now'))",
            params![event.event_id],
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
