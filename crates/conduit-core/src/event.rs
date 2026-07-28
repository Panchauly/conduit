use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub event_id: String,
    pub event_type: String,
    pub payload: String,
    pub metadata: HashMap<String, String>,
}

impl Event {
    pub fn id(&self) -> &str {
        return &self.event_id;
    }
    pub fn event_type(&self) -> &str {
        return &self.event_type;
    }
}
