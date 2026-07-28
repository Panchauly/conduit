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

/// Global routing table (cwd/env dependent). Used by [crate::dispatch::dispatch].
pub(crate) fn global_routing_table() -> &'static HashMap<String, Vec<AdapterId>> {
    &*ROUTING_RULES
}

/// Resolve adapter IDs for an event (uses global routing from env).
pub fn route(event: &Event) -> Vec<AdapterId> {
    route_with_rules(event, global_routing_table())
}

/// Resolve adapter IDs for an event given explicit routing rules (e.g. for explain / dry-run).
///
/// One event type may list several distinct adapters (e.g. `pgsql_master`, `pgsql_slave`); each
/// receives the same logical projection via shared mappings, with execution order by adapter priority.
pub fn route_with_rules(
    event: &Event,
    rules: &HashMap<String, Vec<AdapterId>>,
) -> Vec<AdapterId> {
    rules
        .get(&event.event_type)
        .cloned()
        .unwrap_or_default()
}

/// Explicit loader (used at startup / tests)
pub fn load_routing<P: AsRef<Path>>(path: P) -> Result<HashMap<String, Vec<AdapterId>>, String> {
    let data = fs::read_to_string(path).map_err(|e| format!("Failed to read routing file: {e}"))?;

    serde_json::from_str(&data).map_err(|e| format!("Invalid routing.json: {e}"))
}
