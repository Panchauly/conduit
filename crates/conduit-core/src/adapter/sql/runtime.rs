use std::collections::HashMap;

use super::adapter::SqlError;
use super::mapping::SqlMapping;
use crate::event::Event;
use crate::upcast::UpcasterRegistry;

/// SQL projection plus the version metadata used to produce it (Phase 10.3).
pub struct SqlProjection {
    pub sql: String,
    pub values: Vec<serde_json::Value>,
    pub source_version: u32,
    pub projected_version: u32,
}

/// Owned SQL builder used inside adapters
pub struct SqlRuntimeBuilder {
    mappings: HashMap<String, SqlMapping>,
}

impl SqlRuntimeBuilder {
    pub fn new(mappings: HashMap<String, SqlMapping>) -> Self {
        Self { mappings }
    }

    pub fn get_mapping(&self, event_type: &str) -> Option<&SqlMapping> {
        self.mappings.get(event_type)
    }

    /// Build the SQL projection for `event`, upcasting its payload to the
    /// mapping's target version first when the versions differ.
    pub fn build(
        &self,
        event: &Event,
        upcasters: &UpcasterRegistry,
    ) -> Result<SqlProjection, SqlError> {
        let mapping = self
            .mappings
            .get(&event.event_type)
            .ok_or_else(|| SqlError::MappingNotFound(event.event_type.clone()))?;

        let source_version = event.version();
        let projected_version = mapping.version;

        if source_version == projected_version {
            let (sql, values) = mapping.build(event)?;
            return Ok(SqlProjection {
                sql,
                values,
                source_version,
                projected_version,
            });
        }

        let payload: serde_json::Value = serde_json::from_str(&event.payload)
            .map_err(|e| SqlError::BuildFailed(format!("invalid payload JSON: {}", e)))?;

        let upcasted_payload = upcasters
            .upcast(&event.event_type, payload, source_version, projected_version)
            .map_err(|e| SqlError::UnsupportedVersion {
                event_type: event.event_type.clone(),
                from_version: source_version,
                to_version: projected_version,
                reason: e.to_string(),
            })?;

        let upcasted_event = Event {
            payload: serde_json::to_string(&upcasted_payload).map_err(|e| {
                SqlError::BuildFailed(format!("failed to serialize upcasted payload: {}", e))
            })?,
            version: projected_version,
            ..event.clone()
        };

        let (sql, values) = mapping.build(&upcasted_event)?;
        Ok(SqlProjection {
            sql,
            values,
            source_version,
            projected_version,
        })
    }
}
