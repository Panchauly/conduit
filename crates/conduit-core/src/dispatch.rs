use std::time::SystemTime;

use crate::adapter::StorageAdapter;
use crate::event::Event;
use crate::execution::{AdapterExecutionReport, AdapterReportError, ExecutionReport};
use crate::routing::route;

/// Dispatches an event to routed adapters and returns a single [ExecutionReport].
/// Adapter results are consumed to build the report; nothing adapter-level is returned.
pub fn dispatch(event: &Event, adapters: &mut [Box<dyn StorageAdapter>]) -> ExecutionReport {
    let targets = route(event);

    let trace_id = format!("trace-{}", event.id());
    let started_at = SystemTime::now();

    let mut report = ExecutionReport::new(
        event.id().to_string(),
        event.event_type().to_string(),
        trace_id,
        started_at,
    );

    for adapter in adapters.iter_mut() {
        if !targets.contains(&adapter.id().to_string()) {
            continue;
        }

        let adapter_started_at = SystemTime::now();
        let result = adapter.handle(event);
        let adapter_finished_at = SystemTime::now();

        let adapter_report = AdapterExecutionReport::new(
            adapter.id().to_string(),
            adapter.kind(),
            adapter_started_at,
        );

        let adapter_report = if result.success {
            adapter_report.finish_success(adapter_finished_at)
        } else {
            let err = result.error.as_ref().map(|e| e.into()).unwrap_or(
                AdapterReportError::WriteFailed {
                    message: "unknown error".to_string(),
                },
            );

            adapter_report.finish_failure(adapter_finished_at, err)
        };

        report.push_adapter_report(adapter_report);

        // Fail-fast behavior preserved
        if !result.success {
            return report.finish(adapter_finished_at);
        }
    }

    report.finish(SystemTime::now())
}
