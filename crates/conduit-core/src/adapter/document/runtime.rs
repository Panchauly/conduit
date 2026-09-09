use std::collections::HashMap;

use super::adapter::DocumentError;
use super::mapping::DocumentMapping;
use crate::adapter::OnExisting;
use crate::event::Event;
use crate::upcast::UpcasterRegistry;

/// Document projection plus the version metadata used to produce it (Phase 10.3).
pub struct DocumentProjection {
    pub document: serde_json::Value,
    pub source_version: u32,
    pub projected_version: u32,
    /// Resolved entity identity (Phase 11.1) — keys the idempotency guard and
    /// the output filename (Phase 11.3), rather than `event.event_id`.
    pub entity_id: String,
    /// The mapping's write mode (Phase 12.2) — governs the gated
    /// insert/update/skip decision in the adapter.
    pub on_existing: OnExisting,
}

/// Owned runtime builder (Phase 2)
pub struct DocumentRuntimeBuilder {
    mappings: HashMap<String, DocumentMapping>,
}

impl DocumentRuntimeBuilder {
    pub fn new(mappings: HashMap<String, DocumentMapping>) -> Self {
        Self { mappings }
    }

    /// Build the document projection for `event`, upcasting its payload to the
    /// mapping's target version first when the versions differ.
    pub fn build(
        &self,
        event: &Event,
        upcasters: &UpcasterRegistry,
    ) -> Result<DocumentProjection, DocumentError> {
        let mapping = self
            .mappings
            .get(&event.event_type)
            .ok_or_else(|| DocumentError::MappingNotFound(event.event_type.clone()))?;

        let source_version = event.version();
        let projected_version = mapping.version;

        if source_version == projected_version {
            let (document, entity_id) = mapping.apply(event)?;
            return Ok(DocumentProjection {
                document,
                entity_id,
                source_version,
                projected_version,
                on_existing: mapping.on_existing,
            });
        }

        let payload: serde_json::Value = serde_json::from_str(&event.payload)
            .map_err(|e| DocumentError::BuildFailed(format!("invalid payload JSON: {}", e)))?;

        let upcasted_payload = upcasters
            .upcast(
                &event.event_type,
                payload,
                source_version,
                projected_version,
            )
            .map_err(|e| DocumentError::UnsupportedVersion {
                event_type: event.event_type.clone(),
                from_version: source_version,
                to_version: projected_version,
                reason: e.to_string(),
            })?;

        let upcasted_event = Event {
            payload: serde_json::to_string(&upcasted_payload).map_err(|e| {
                DocumentError::BuildFailed(format!("failed to serialize upcasted payload: {}", e))
            })?,
            version: projected_version,
            ..event.clone()
        };

        let (document, entity_id) = mapping.apply(&upcasted_event)?;
        Ok(DocumentProjection {
            document,
            entity_id,
            source_version,
            projected_version,
            on_existing: mapping.on_existing,
        })
    }
}
