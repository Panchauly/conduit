use std::collections::HashMap;

#[derive(Debug)]
pub struct Event {
    pub event_type: String,
    pub payload: String,
    pub metadata: HashMap<String, String>,
}
