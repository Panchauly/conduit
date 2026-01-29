use rusqlite::{Connection, params, params_from_iter};
use serde_json::Value;

use crate::{
    adapter::sql::runtime::SqlRuntimeBuilder,
    adapter::{AdapterError, AdapterResult, StorageAdapter},
    event::Event,
    routing::StorageKind,
};

/// SQLite runtime adapter (Phase 4: idempotent)
pub struct SqliteAdapter {
    id: String,
    priority: u32,
    path: String,
    builder: SqlRuntimeBuilder,
}

impl SqliteAdapter {
    pub fn new(id: String, path: String, priority: u32, builder: SqlRuntimeBuilder) -> Self {
        Self {
            id,
            path,
            priority,
            builder,
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
        // Build SQL projection
        let (sql, values) = match self.builder.build(event) {
            Ok(v) => v,
            Err(e) => {
                return AdapterResult {
                    adapter_id: self.id.clone(),
                    kind: StorageKind::Sql,
                    success: false,
                    error: Some(AdapterError::WriteFailed(e.to_string())),
                };
            }
        };

        let mut conn = match Connection::open(&self.path) {
            Ok(c) => c,
            Err(e) => {
                return AdapterResult {
                    adapter_id: self.id.clone(),
                    kind: StorageKind::Sql,
                    success: false,
                    error: Some(AdapterError::WriteFailed(e.to_string())),
                };
            }
        };

        // PHASE 4: ensure idempotency table exists
        if let Err(e) = conn.execute(
            "CREATE TABLE IF NOT EXISTS conduit_events (
                event_id TEXT PRIMARY KEY,
                processed_at TEXT NOT NULL
            )",
            [],
        ) {
            return AdapterResult {
                adapter_id: self.id.clone(),
                kind: StorageKind::Sql,
                success: false,
                error: Some(AdapterError::WriteFailed(e.to_string())),
            };
        }

        let tx = match conn.transaction() {
            Ok(tx) => tx,
            Err(e) => {
                return AdapterResult {
                    adapter_id: self.id.clone(),
                    kind: StorageKind::Sql,
                    success: false,
                    error: Some(AdapterError::WriteFailed(e.to_string())),
                };
            }
        };

        // PHASE 4: idempotency check
        let already_processed: bool = match tx.query_row(
            "SELECT 1 FROM conduit_events WHERE event_id = ?",
            params![event.event_id],
            |_| Ok(()),
        ) {
            Ok(_) => true,
            Err(rusqlite::Error::QueryReturnedNoRows) => false,
            Err(e) => {
                return AdapterResult {
                    adapter_id: self.id.clone(),
                    kind: StorageKind::Sql,
                    success: false,
                    error: Some(AdapterError::WriteFailed(e.to_string())),
                };
            }
        };

        if already_processed {
            return AdapterResult {
                adapter_id: self.id.clone(),
                kind: StorageKind::Sql,
                success: true,
                error: Some(AdapterError::Skipped("event already processed".into())),
            };
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
            return AdapterResult {
                adapter_id: self.id.clone(),
                kind: StorageKind::Sql,
                success: false,
                error: Some(AdapterError::WriteFailed(e.to_string())),
            };
        }

        // PHASE 4: record event as processed
        if let Err(e) = tx.execute(
            "INSERT INTO conduit_events (event_id, processed_at)
             VALUES (?, datetime('now'))",
            params![event.event_id],
        ) {
            return AdapterResult {
                adapter_id: self.id.clone(),
                kind: StorageKind::Sql,
                success: false,
                error: Some(AdapterError::WriteFailed(e.to_string())),
            };
        }

        if let Err(e) = tx.commit() {
            return AdapterResult {
                adapter_id: self.id.clone(),
                kind: StorageKind::Sql,
                success: false,
                error: Some(AdapterError::WriteFailed(e.to_string())),
            };
        }

        AdapterResult {
            adapter_id: self.id.clone(),
            kind: StorageKind::Sql,
            success: true,
            error: None,
        }
    }
}
