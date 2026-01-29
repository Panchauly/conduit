pub mod adapter;
pub mod dispatch;
pub mod event;
pub mod routing;
pub mod runtime;

use std::collections::HashMap;

use crate::adapter::AdapterResult;
use crate::adapter::document::mapping::DocumentMapping;
use crate::adapter::sql::mapping::SqlMapping;
use crate::dispatch::dispatch;
use crate::event::Event;
use crate::runtime::factory::build_adapters;

/// Execute a single event using default adapters.
///
/// Public, stable API (Phase 5).
///
/// Guarantees:
/// - routing is config-backed
/// - adapters are built via the runtime factory
/// - failures are explicit in results
/// - no retries, no rollback
pub fn execute_event(
    sql_mappings: HashMap<String, SqlMapping>,
    document_mappings: HashMap<String, DocumentMapping>,
    event: Event,
) -> Vec<AdapterResult> {
    let mut adapters = build_adapters(sql_mappings, document_mappings);
    dispatch(&event, &mut adapters)
}
