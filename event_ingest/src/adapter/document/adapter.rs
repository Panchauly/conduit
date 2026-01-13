use std::collections::HashMap;

use serde_json::{Map, Value};

use crate::{
    adapter::{AdapterError, StorageAdapter, document::mapping::DocumentMapping},
    event::Event,
    routing::StorageKind,
};

pub struct DocumentAdapter {
    mappings: HashMap<String, DocumentMapping>,
}

impl DocumentAdapter {
    pub fn new(mappings: HashMap<String, DocumentMapping>) -> Self {
        Self { mappings }
    }
}

impl StorageAdapter for DocumentAdapter {
    fn kind(&self) -> StorageKind {
        StorageKind::Document
    }

    fn handle(&self, event: &Event) -> Result<(), AdapterError> {
        let mapping = self.mappings.get(&event.event_type).ok_or_else(|| {
            AdapterError::WriteFailed(format!(
                "No document mapping for event '{}'",
                event.event_type
            ))
        })?;

        let document =
            build_document(event, &mapping.document).map_err(AdapterError::WriteFailed)?;

        // Simulated document DB write
        println!(
            "DOCUMENT INSERT → collection='{}' document={}",
            mapping.collection, document
        );

        Ok(())
    }
}

fn extract_value(event: &Event, path: &str) -> Result<Value, String> {
    let (root, rest) = if let Some(p) = path.strip_prefix("payload.") {
        (
            serde_json::from_str::<Value>(&event.payload)
                .map_err(|e| format!("Invalid payload JSON: {}", e))?,
            p,
        )
    } else if let Some(p) = path.strip_prefix("metadata.") {
        (
            serde_json::to_value(&event.metadata)
                .map_err(|e| format!("Invalid metadata: {}", e))?,
            p,
        )
    } else {
        return Err(format!("Invalid path '{}'", path));
    };

    let mut current = &root;
    for part in rest.split('.') {
        current = current
            .get(part)
            .ok_or(format!("Missing field '{}' in '{}'", part, path))?;
    }

    Ok(current.clone())
}

fn build_document(event: &Event, template: &Value) -> Result<Value, String> {
    match template {
        Value::String(path) => extract_value(event, path),

        Value::Object(map) => {
            let mut result = Map::new();
            for (k, v) in map {
                let value = build_document(event, v)?;
                result.insert(k.clone(), value);
            }
            Ok(Value::Object(result))
        }

        _ => Err("Document template must contain only strings or objects".to_string()),
    }
}
