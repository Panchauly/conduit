pub mod adapter;
pub mod dispatch;
pub mod event;
pub mod execution;
pub mod pipeline;
pub mod replay;
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
    AdapterExecutionReport, AdapterOutcome, AdapterReportError, ExecutionMode, ExecutionReport,
    ExecutionStatus,
};

/// Phase 7 replay API.
pub use replay::{
    events_from_path, replay_stream, replay_stream_with_options, EventsFromPath,
    PerEventReplaySummary, ReplayContext, ReplayLoadError, ReplayReport, ReplayRunOptions,
};

/// Phase 8 projection validation.
pub use runtime::{
    adapter_metadata_map, dependency_depth_exceeds_recommended, dependency_layers_grouped,
    dependency_layers_parallel, execution_order_for_routed, max_dependency_layer,
    validate_projection_config, validate_routing_adapter_ids,
    validate_routing_and_dependencies_for_event_type, validate_routing_for_event_type,
    AdapterExecutionMeta, ValidationIssue, ValidationReport,
    PROJECTION_DEPTH_EXCEEDS_RECOMMENDED_WARNING, RECOMMENDED_MAX_DEPENDENCY_LAYER,
};

/// Execution report types (also at [crate root](crate) for convenience).
pub mod report {
    pub use crate::execution::{
        AdapterExecutionReport, AdapterOutcome, AdapterReportError, ExecutionMode, ExecutionReport,
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
/// - call [`validate_projection_config`] at startup when config + routing + mappings are loaded (CLI does this for run/replay/dry-run)
/// - mappings are consumed exactly once
/// - routing remains config-backed
pub fn execute_event(
    config: &ConduitConfig,
    sql_mappings: HashMap<String, SqlMapping>,
    document_mappings: HashMap<String, DocumentMapping>,
    event: Event,
) -> ExecutionReport {
    let mut adapters = build_adapters_from_config(config, sql_mappings, document_mappings);
    let adapter_meta = crate::runtime::adapter_metadata_map(config);
    dispatch(
        &event,
        &mut adapters,
        config.failure_policy,
        &adapter_meta,
    )
}

/// Execute a single event with an explicit [ExecutionMode] (e.g. [execution::ExecutionMode::DryRun] for no-write simulation).
/// Same as [execute_event] when mode is [execution::ExecutionMode::Run].
/// When mode is DryRun, the pipeline runs but adapters are expected to skip persistence (adapter support is required for true dry-run).
pub fn execute_event_with_mode(
    config: &ConduitConfig,
    sql_mappings: HashMap<String, SqlMapping>,
    document_mappings: HashMap<String, DocumentMapping>,
    event: Event,
    mode: crate::execution::ExecutionMode,
) -> ExecutionReport {
    match mode {
        crate::execution::ExecutionMode::Run => {
            execute_event(config, sql_mappings, document_mappings, event)
        }
        crate::execution::ExecutionMode::DryRun => {
            let mut adapters = build_adapters_from_config(config, sql_mappings, document_mappings);
            let adapter_meta = crate::runtime::adapter_metadata_map(config);
            dispatch(
                &event,
                &mut adapters,
                config.failure_policy,
                &adapter_meta,
            )
        }
    }
}
