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
    /// Resolved entity identity (Phase 11.1/11.2), canonicalized as an
    /// ordered JSON array of the `primary_key` column value(s) in declared
    /// order — `["u1"]` for a single-column key, `["a","b"]` for a composite
    /// one. One encoding path for both cases, so a single-column key never
    /// collides with a differently-split composite key.
    pub entity_key: String,
    /// Target table name, carried alongside `entity_key` as the other half of
    /// the Phase 11.2 guard key (`conduit_projection_state` is keyed per table).
    pub table: String,
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
            let (sql, values, key_values) = mapping.build(event)?;
            return Ok(SqlProjection {
                sql,
                values,
                source_version,
                projected_version,
                entity_key: encode_entity_key(&key_values)?,
                table: mapping.table.clone(),
            });
        }

        let payload: serde_json::Value = serde_json::from_str(&event.payload)
            .map_err(|e| SqlError::BuildFailed(format!("invalid payload JSON: {}", e)))?;

        let upcasted_payload = upcasters
            .upcast(
                &event.event_type,
                payload,
                source_version,
                projected_version,
            )
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

        let (sql, values, key_values) = mapping.build(&upcasted_event)?;
        Ok(SqlProjection {
            sql,
            values,
            source_version,
            projected_version,
            entity_key: encode_entity_key(&key_values)?,
            table: mapping.table.clone(),
        })
    }
}

/// Canonical entity-key encoding (Phase 11.1): an ordered JSON array of the
/// resolved `primary_key` value(s), single-column keys included — exactly one
/// encoding path, so naive concatenation can never make `("AB","C")` collide
/// with `("A","BC")`.
fn encode_entity_key(key_values: &[serde_json::Value]) -> Result<String, SqlError> {
    serde_json::to_string(key_values)
        .map_err(|e| SqlError::BuildFailed(format!("failed to encode entity key: {}", e)))
}
