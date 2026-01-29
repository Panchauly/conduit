use std::collections::HashMap;

use super::adapter::SqlError;
use super::mapping::SqlMapping;
use crate::event::Event;

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

    pub fn build(&self, event: &Event) -> Result<(String, Vec<serde_json::Value>), SqlError> {
        let mapping = self
            .mappings
            .get(&event.event_type)
            .ok_or_else(|| SqlError::MappingNotFound(event.event_type.clone()))?;

        mapping.build(event)
    }
}
