use std::collections::HashMap;

use serde::{Deserialize, Serialize};

fn default_event_version() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub event_id: String,
    pub event_type: String,
    pub payload: String,
    pub metadata: HashMap<String, String>,
    /// Payload schema version. Unversioned (legacy) events default to 1.
    #[serde(default = "default_event_version")]
    pub version: u32,
}

impl Event {
    pub fn id(&self) -> &str {
        &self.event_id
    }
    pub fn event_type(&self) -> &str {
        &self.event_type
    }
    pub fn version(&self) -> u32 {
        self.version
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unversioned_legacy_json_defaults_to_version_one() {
        let json = r#"{
            "event_id": "e1",
            "event_type": "UserCreated",
            "payload": "{}",
            "metadata": {}
        }"#;
        let event: Event = serde_json::from_str(json).unwrap();
        assert_eq!(event.version(), 1);
    }

    #[test]
    fn explicit_version_is_preserved() {
        let json = r#"{
            "event_id": "e1",
            "event_type": "UserCreated",
            "payload": "{}",
            "metadata": {},
            "version": 3
        }"#;
        let event: Event = serde_json::from_str(json).unwrap();
        assert_eq!(event.version(), 3);
    }
}
