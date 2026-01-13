use std::collections::HashMap;

use serde_json::Value;

use crate::adapter::document::mapping::DocumentMapping;
use crate::routing::StorageKind;

fn validate_document_paths(value: &Value) -> Result<(), String> {
    match value {
        Value::String(path) => {
            if path.starts_with("payload.") || path.starts_with("metadata.") {
                Ok(())
            } else {
                Err(format!(
                    "Invalid document path '{}'. Must start with 'payload.' or 'metadata.'",
                    path
                ))
            }
        }

        Value::Object(map) => {
            for (_, v) in map {
                validate_document_paths(v)?;
            }
            Ok(())
        }

        _ => Err("Document mapping values must be strings or objects".to_string()),
    }
}

pub fn validate_document_mappings(
    routing: &HashMap<String, Vec<StorageKind>>,
    document_mappings: &HashMap<String, DocumentMapping>,
) -> Result<(), String> {
    // Rule A: routing → mapping
    for (event, targets) in routing {
        if targets.contains(&StorageKind::Document) {
            if !document_mappings.contains_key(event) {
                return Err(format!(
                    "Routing includes Document for event '{}' but no document mapping found",
                    event
                ));
            }
        }
    }

    // Rule B, C, D
    for (event, mapping) in document_mappings {
        match routing.get(event) {
            Some(targets) if targets.contains(&StorageKind::Document) => {}
            Some(_) => {
                return Err(format!(
                    "Document mapping exists for event '{}' but routing does not include Document",
                    event
                ));
            }
            None => {
                return Err(format!(
                    "Document mapping exists for event '{}' but event is not in routing.json",
                    event
                ));
            }
        }

        // Rule C: _id must exist
        let doc = mapping
            .document
            .as_object()
            .ok_or("Document root must be an object")?;

        let id = doc
            .get("_id")
            .ok_or("Document mapping must contain '_id' field")?;

        if !id.is_string() {
            return Err("Document '_id' must be a string path".to_string());
        }

        // Rule D: validate all paths
        validate_document_paths(&mapping.document)?;
    }

    Ok(())
}
