use std::collections::HashMap;

use serde::Deserialize;

use super::adapter::SqlError;
use crate::event::Event;
use crate::runtime::config::AdapterCapability;
use serde_json::Value;

#[derive(Debug, Clone, Deserialize)]
pub struct SqlMapping {
    pub event: String,
    pub table: String,

    /// Target schema version this mapping projects into. Must be explicit ($\ge 1$).
    pub version: u32,

    pub primary_key: String,

    pub columns: HashMap<String, String>,

    #[serde(default)]
    pub foreign_keys: HashMap<String, String>,

    /// Capabilities adapters must provide to run this projection (enum; parse-time validated).
    #[serde(default)]
    pub requires_capabilities: Vec<AdapterCapability>,
}

impl SqlMapping {
    pub fn build(&self, event: &Event) -> Result<(String, Vec<Value>), SqlError> {
        // Parse payload JSON
        let payload: Value = serde_json::from_str(&event.payload)
            .map_err(|e| SqlError::BuildFailed(format!("invalid payload JSON: {}", e)))?;

        let payload_obj = payload
            .as_object()
            .ok_or_else(|| SqlError::BuildFailed("payload must be a JSON object".into()))?;

        // Deterministic column order (important!)
        let mut column_pairs: Vec<(&String, &String)> = self.columns.iter().collect();

        column_pairs.sort_by(|a, b| a.0.cmp(b.0));

        let mut columns = Vec::with_capacity(column_pairs.len());
        let mut values = Vec::with_capacity(column_pairs.len());

        for (column, path_expr) in column_pairs {
            let value = self.resolve_path(path_expr, payload_obj, &event.metadata)?;

            columns.push(column.clone());
            values.push(value);
        }

        let placeholders = vec!["?"; columns.len()].join(", ");

        let sql = format!(
            "INSERT INTO {} ({}) VALUES ({})",
            self.table,
            columns.join(", "),
            placeholders
        );

        Ok((sql, values))
    }

    fn resolve_path(
        &self,
        path: &str,
        payload: &serde_json::Map<String, Value>,
        metadata: &HashMap<String, String>,
    ) -> Result<Value, SqlError> {
        if let Some(rest) = path.strip_prefix("payload.") {
            payload.get(rest).cloned().ok_or_else(|| {
                SqlError::BuildFailed(format!("payload field '{}' not found", rest))
            })
        } else if let Some(rest) = path.strip_prefix("metadata.") {
            metadata
                .get(rest)
                .map(|v| Value::String(v.clone()))
                .ok_or_else(|| {
                    SqlError::BuildFailed(format!("metadata field '{}' not found", rest))
                })
        } else {
            Err(SqlError::BuildFailed(format!(
                "invalid path '{}' - must start with 'payload.' or 'metadata.'",
                path
            )))
        }
    }
}
