pub mod adapter;
pub mod dispatch;
pub mod event;
pub mod routing;
pub mod runtime;

use std::collections::HashMap;

use crate::{
    adapter::{AdapterResult, document::mapping::DocumentMapping, sql::mapping::SqlMapping},
    dispatch::dispatch,
    event::Event,
    runtime::{build_adapters_from_config, config::ConduitConfig},
};

/// Execute a single event using a validated configuration.
///
/// Phase 5 public API.
///
/// Requirements:
/// - `config.validate()` MUST be called before this function
/// - mappings are consumed exactly once
/// - routing remains config-backed
pub fn execute_event(
    config: &ConduitConfig,
    sql_mappings: HashMap<String, SqlMapping>,
    document_mappings: HashMap<String, DocumentMapping>,
    event: Event,
) -> Vec<AdapterResult> {
    let mut adapters = build_adapters_from_config(config, sql_mappings, document_mappings);

    dispatch(&event, &mut adapters)
}
