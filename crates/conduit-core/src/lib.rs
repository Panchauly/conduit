pub mod adapter;
pub mod dispatch;
pub mod event;
pub mod execution;
pub mod routing;
pub mod runtime;

use std::collections::HashMap;

use crate::{
    adapter::{document::mapping::DocumentMapping, sql::mapping::SqlMapping},
    dispatch::dispatch,
    event::Event,
    runtime::{build_adapters_from_config, config::ConduitConfig},
};

// ---------------------------------------------------------------------------
// Public API: execution contract (stable)
// ---------------------------------------------------------------------------
//
// [ExecutionReport] is the official return type of [execute_event]. It is not
// an internal artifact or transitional type. Consumers depend on this contract.

/// Re-export at crate root so users can `use conduit_core::ExecutionReport`.
pub use execution::{
    AdapterExecutionReport, AdapterOutcome, AdapterReportError, ExecutionReport,
    ExecutionStatus,
};

/// Execution report types (also at [crate root](crate) for convenience).
pub mod report {
    pub use crate::execution::{
        AdapterExecutionReport, AdapterOutcome, AdapterReportError, ExecutionReport,
        ExecutionStatus,
    };
}

// ---------------------------------------------------------------------------
// Execution entry point
// ---------------------------------------------------------------------------

/// Execute a single event using a validated configuration.
///
/// **Stable contract:** returns an [ExecutionReport]. This is the official
/// public API for execution — not internal, not incidental. The report is
/// the single contract for observability and outcome.
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
) -> ExecutionReport {
    let mut adapters = build_adapters_from_config(config, sql_mappings, document_mappings);

    dispatch(&event, &mut adapters)
}
