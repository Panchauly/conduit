pub mod loader;
pub mod mapping;
pub mod validate;

use std::collections::HashMap;

use serde_json::Value;

use crate::{
    adapter::{AdapterError, StorageAdapter, sql::mapping::SqlMapping},
    event::Event,
    routing::StorageKind,
};

pub struct SqlAdapter {
    mappings: HashMap<String, SqlMapping>,
}

impl SqlAdapter {
    pub fn new(mappings: HashMap<String, SqlMapping>) -> Self {
        Self { mappings }
    }
}

fn extract_value(payload: &str, path: &str) -> Result<Value, String> {
    let json: Value =
        serde_json::from_str(payload).map_err(|e| format!("Invalid JSON payload: {}", e))?;

    let mut current = &json;

    let path = path
        .strip_prefix("payload.")
        .ok_or("Path must start with 'payload.'")?;

    for part in path.split('.') {
        current = current
            .get(part)
            .ok_or(format!("Missing field '{}' in payload", part))?;
    }

    Ok(current.clone())
}

impl StorageAdapter for SqlAdapter {
    fn kind(&self) -> StorageKind {
        StorageKind::Sql
    }

    fn handle(&self, event: &Event) -> Result<(), AdapterError> {
        let mapping = self.mappings.get(&event.event_type).ok_or_else(|| {
            AdapterError::WriteFailed(format!("No SQL mapping for event '{}'", event.event_type))
        })?;

        let mut columns = Vec::new();
        let mut values = Vec::new();

        for (column, path) in &mapping.columns {
            let value = extract_value(&event.payload, path).map_err(AdapterError::WriteFailed)?;

            columns.push(column.clone());
            values.push(value);
        }

        let placeholders = (0..values.len())
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(", ");

        let sql = format!(
            "INSERT INTO {} ({}) VALUES ({})",
            mapping.table,
            columns.join(", "),
            placeholders
        );

        // Simulated execution
        println!("SQL: {}", sql);
        println!("Values: {:?}", values);

        Ok(())
    }
}
