use std::collections::HashMap;
use std::fs;
use std::path::Path;

use once_cell::sync::Lazy;
use serde::Deserialize;

use crate::event::Event;

#[derive(Debug, PartialEq, Clone, Deserialize)]
pub enum StorageKind {
    Sql,
    Document,
    KeyValue,
    Graph,
}

static ROUTING_RULES: Lazy<HashMap<String, Vec<StorageKind>>> = Lazy::new(|| {
    let path = std::env::var("ROUTING_CONFIG").unwrap_or_else(|_| "routing.json".to_string());

    let data = fs::read_to_string(path).expect("Failed to read routing.json");

    serde_json::from_str(&data).expect("Invalid routing.json format")
});

pub fn route(event: &Event) -> Vec<StorageKind> {
    ROUTING_RULES
        .get(&event.event_type)
        .cloned()
        .unwrap_or_else(|| vec![StorageKind::Document])
}

pub fn load_routing<P: AsRef<Path>>(path: P) -> Result<HashMap<String, Vec<StorageKind>>, String> {
    let data =
        fs::read_to_string(path).map_err(|e| format!("Failed to read routing file: {}", e))?;

    serde_json::from_str(&data).map_err(|e| format!("Invalid routing.json: {}", e))
}
