use core::fmt;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};

use crate::event::Event;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageKind {
    Sql,
    Document,
    KeyValue,
    Graph,
}

pub type AdapterId = String;

impl fmt::Display for StorageKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            StorageKind::Sql => "sql",
            StorageKind::Document => "document",
            Self::KeyValue => "keyvalue",
            Self::Graph => "graph",
        };
        write!(f, "{s}")
    }
}

static ROUTING_RULES: Lazy<HashMap<String, Vec<AdapterId>>> = Lazy::new(|| {
    let path = std::env::var("ROUTING_CONFIG").unwrap_or_else(|_| "routing.json".to_string());

    let data = fs::read_to_string(path).expect("Failed to read routing.json");

    serde_json::from_str(&data).expect("Invalid routing.json format")
});

/// Resolve adapter IDs for an event
pub fn route(event: &Event) -> Vec<AdapterId> {
    ROUTING_RULES
        .get(&event.event_type)
        .cloned()
        .unwrap_or_default()
}

/// Explicit loader (used at startup / tests)
pub fn load_routing<P: AsRef<Path>>(path: P) -> Result<HashMap<String, Vec<AdapterId>>, String> {
    let data = fs::read_to_string(path).map_err(|e| format!("Failed to read routing file: {e}"))?;

    serde_json::from_str(&data).map_err(|e| format!("Invalid routing.json: {e}"))
}
