use super::adapter::DocumentError;
use crate::event::Event;
use crate::runtime::config::AdapterCapability;
use serde::Deserialize;
use serde_json::{Map, Value};

#[derive(Debug, Clone, Deserialize)]
pub struct DocumentMapping {
    pub event: String,
    pub collection: String,

    /// JSON-like structure where leaf values are payload/metadata paths
    pub document: Value,

    /// Capabilities adapters must provide to run this projection.
    #[serde(default)]
    pub requires_capabilities: Vec<AdapterCapability>,
}

impl DocumentMapping {
    /// Build document JSON from event payload + metadata
    pub fn apply(&self, event: &Event) -> Result<Value, DocumentError> {
        // Parse payload JSON
        let payload: Value = serde_json::from_str(&event.payload)
            .map_err(|e| DocumentError::BuildFailed(format!("invalid payload JSON: {}", e)))?;

        let payload_obj = payload
            .as_object()
            .ok_or_else(|| DocumentError::BuildFailed("payload must be a JSON object".into()))?;

        // Expand template recursively
        self.expand_value(&self.document, payload_obj, &event.metadata)
    }

    fn expand_value(
        &self,
        template: &Value,
        payload: &Map<String, Value>,
        metadata: &std::collections::HashMap<String, String>,
    ) -> Result<Value, DocumentError> {
        match template {
            Value::Object(map) => {
                let mut out = Map::new();
                for (k, v) in map {
                    out.insert(k.clone(), self.expand_value(v, payload, metadata)?);
                }
                Ok(Value::Object(out))
            }

            Value::Array(arr) => {
                let mut out = Vec::with_capacity(arr.len());
                for v in arr {
                    out.push(self.expand_value(v, payload, metadata)?);
                }
                Ok(Value::Array(out))
            }

            Value::String(path) => {
                // Interpret as path expression
                self.resolve_path(path, payload, metadata)
            }

            // Literal value (number, bool, null)
            other => Ok(other.clone()),
        }
    }

    fn resolve_path(
        &self,
        path: &str,
        payload: &Map<String, Value>,
        metadata: &std::collections::HashMap<String, String>,
    ) -> Result<Value, DocumentError> {
        if let Some(rest) = path.strip_prefix("payload.") {
            payload.get(rest).cloned().ok_or_else(|| {
                DocumentError::BuildFailed(format!("payload field '{}' not found", rest))
            })
        } else if let Some(rest) = path.strip_prefix("metadata.") {
            metadata
                .get(rest)
                .map(|v| Value::String(v.clone()))
                .ok_or_else(|| {
                    DocumentError::BuildFailed(format!("metadata field '{}' not found", rest))
                })
        } else {
            Err(DocumentError::BuildFailed(format!(
                "invalid document path '{}'",
                path
            )))
        }
    }
}
