use std::collections::HashMap;

use super::adapter::DocumentError;
use super::mapping::DocumentMapping;
use crate::event::Event;

/// Owned runtime builder (Phase 2)
pub struct DocumentRuntimeBuilder {
    mappings: HashMap<String, DocumentMapping>,
}

impl DocumentRuntimeBuilder {
    pub fn new(mappings: HashMap<String, DocumentMapping>) -> Self {
        Self { mappings }
    }

    pub fn build(&self, event: &Event) -> Result<serde_json::Value, DocumentError> {
        let mapping = self
            .mappings
            .get(&event.event_type)
            .ok_or_else(|| DocumentError::MappingNotFound(event.event_type.clone()))?;

        mapping.apply(event)
    }
}
