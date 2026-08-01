//! Execution reporting types for observability and audit.
//!
//! All timestamps are serialized as milliseconds since Unix epoch.
//! Time is injected by the runtime — never generated inside report structs.
//! Reports are deterministic in structure; timestamps are runtime-controlled.

use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use crate::adapter::AdapterError;
use crate::routing::StorageKind;

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

/// Overall status of a single event dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStatus {
    Succeeded,
    Failed,
}

/// Execution mode: normal (writes) or dry-run (no side effects).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionMode {
    /// Normal execution; adapters perform writes.
    Run,
    /// Simulated execution; adapters should not persist (no writes).
    DryRun,
}

/// Outcome of a single adapter invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdapterOutcome {
    Succeeded,
    /// Write or side-effect failed.
    WriteFailed,
    /// Idempotent replay (event already applied).
    Skipped,
}

/// Structured error for adapter execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AdapterReportError {
    WriteFailed { message: String },
    Skipped { message: String },
}

impl From<AdapterError> for AdapterReportError {
    fn from(e: AdapterError) -> Self {
        match e {
            AdapterError::WriteFailed(msg) => AdapterReportError::WriteFailed { message: msg },
            AdapterError::Skipped(msg) => AdapterReportError::Skipped { message: msg },
        }
    }
}

impl From<&AdapterError> for AdapterReportError {
    fn from(e: &AdapterError) -> Self {
        match e {
            AdapterError::WriteFailed(msg) => AdapterReportError::WriteFailed {
                message: msg.clone(),
            },
            AdapterError::Skipped(msg) => AdapterReportError::Skipped {
                message: msg.clone(),
            },
        }
    }
}

// ---------------------------------------------------------------------------
// ExecutionReport (event-level)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionReport {
    /// Schema version for forward evolution.
    #[serde(default = "default_report_version")]
    pub report_version: String,

    pub event_id: String,
    pub event_type: String,
    pub trace_id: String,

    #[serde(with = "crate::execution::time")]
    pub started_at: SystemTime,

    #[serde(with = "crate::execution::time_option")]
    pub finished_at: Option<SystemTime>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,

    /// Adapter reports in execution order.
    pub adapter_reports: Vec<AdapterExecutionReport>,

    pub status: ExecutionStatus,
}

fn default_report_version() -> String {
    "1.0".to_string()
}

impl ExecutionReport {
    /// Create a new report with injected start time.
    pub fn new(
        event_id: String,
        event_type: String,
        trace_id: String,
        started_at: SystemTime,
    ) -> Self {
        Self {
            report_version: default_report_version(),
            event_id,
            event_type,
            trace_id,
            started_at,
            finished_at: None,
            duration_ms: None,
            adapter_reports: Vec::new(),
            status: ExecutionStatus::Succeeded, // provisional, recalculated on finish
        }
    }

    /// Add adapter report (preserves order).
    pub fn push_adapter_report(&mut self, report: AdapterExecutionReport) {
        self.adapter_reports.push(report);
    }

    /// Finalize report and compute duration + status.
    pub fn finish(mut self, finished_at: SystemTime) -> Self {
        self.finished_at = Some(finished_at);

        self.duration_ms = finished_at
            .duration_since(self.started_at)
            .ok()
            .map(duration_to_ms);

        self.status = if self
            .adapter_reports
            .iter()
            .any(|r| r.outcome == AdapterOutcome::WriteFailed)
        {
            ExecutionStatus::Failed
        } else {
            ExecutionStatus::Succeeded
        };

        self
    }
}

// ---------------------------------------------------------------------------
// AdapterExecutionReport (adapter-level)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdapterExecutionReport {
    /// Adapter id from config.
    pub adapter_id: String,

    /// Storage kind (sql, document, etc.).
    pub storage_kind: StorageKind,

    #[serde(with = "crate::execution::time")]
    pub started_at: SystemTime,

    #[serde(with = "crate::execution::time_option")]
    pub finished_at: Option<SystemTime>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,

    pub outcome: AdapterOutcome,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<AdapterReportError>,
}

impl AdapterExecutionReport {
    /// Create new adapter report with injected start time.
    pub fn new(adapter_id: String, storage_kind: StorageKind, started_at: SystemTime) -> Self {
        Self {
            adapter_id,
            storage_kind,
            started_at,
            finished_at: None,
            duration_ms: None,
            outcome: AdapterOutcome::Succeeded, // provisional
            error: None,
        }
    }

    /// Finish successfully.
    pub fn finish_success(mut self, finished_at: SystemTime) -> Self {
        self.finished_at = Some(finished_at);

        self.duration_ms = finished_at
            .duration_since(self.started_at)
            .ok()
            .map(duration_to_ms);

        self.outcome = AdapterOutcome::Succeeded;

        self
    }

    /// Finish with failure or skipped.
    pub fn finish_failure(mut self, finished_at: SystemTime, error: AdapterReportError) -> Self {
        self.finished_at = Some(finished_at);

        self.duration_ms = finished_at
            .duration_since(self.started_at)
            .ok()
            .map(duration_to_ms);

        self.outcome = match &error {
            AdapterReportError::WriteFailed { .. } => AdapterOutcome::WriteFailed,
            AdapterReportError::Skipped { .. } => AdapterOutcome::Skipped,
        };

        self.error = Some(error);

        self
    }
}

// ---------------------------------------------------------------------------
// Utilities
// ---------------------------------------------------------------------------

fn duration_to_ms(d: Duration) -> u64 {
    d.as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_report_has_provisional_succeeded_status() {
        let started = SystemTime::now();
        let report = ExecutionReport::new(
            "evt-1".to_string(),
            "TestEvent".to_string(),
            "trace-1".to_string(),
            started,
        );

        assert_eq!(report.status, ExecutionStatus::Succeeded);
        assert!(report.finished_at.is_none());
        assert!(report.duration_ms.is_none());
        assert!(report.adapter_reports.is_empty());
        assert_eq!(report.report_version, "1.0");
    }

    #[test]
    fn finish_computes_duration_and_succeeded_status() {
        let started = SystemTime::now();
        let finished = started + Duration::from_millis(250);

        let mut report =
            ExecutionReport::new("e".to_string(), "T".to_string(), "trace".to_string(), started);
        report.push_adapter_report(
            AdapterExecutionReport::new("sql".to_string(), StorageKind::Sql, started)
                .finish_success(finished),
        );
        let report = report.finish(finished);

        assert_eq!(report.status, ExecutionStatus::Succeeded);
        assert_eq!(report.duration_ms, Some(250));
        assert_eq!(report.finished_at, Some(finished));
    }

    #[test]
    fn finish_marks_failed_when_any_adapter_write_failed() {
        let started = SystemTime::now();
        let finished = started + Duration::from_millis(10);

        let mut report =
            ExecutionReport::new("e".to_string(), "T".to_string(), "trace".to_string(), started);
        report.push_adapter_report(
            AdapterExecutionReport::new("sql".to_string(), StorageKind::Sql, started)
                .finish_success(finished),
        );
        report.push_adapter_report(
            AdapterExecutionReport::new("doc".to_string(), StorageKind::Document, started)
                .finish_failure(
                    finished,
                    AdapterReportError::WriteFailed {
                        message: "boom".to_string(),
                    },
                ),
        );
        let report = report.finish(finished);

        assert_eq!(report.status, ExecutionStatus::Failed);
    }

    #[test]
    fn finish_failure_with_skipped_error_does_not_fail_overall_status() {
        let started = SystemTime::now();

        let mut report =
            ExecutionReport::new("e".to_string(), "T".to_string(), "trace".to_string(), started);
        let adapter = AdapterExecutionReport::new("doc".to_string(), StorageKind::Document, started)
            .finish_failure(
                started,
                AdapterReportError::Skipped {
                    message: "already applied".to_string(),
                },
            );

        assert_eq!(adapter.outcome, AdapterOutcome::Skipped);

        report.push_adapter_report(adapter);
        let report = report.finish(started);

        assert_eq!(report.status, ExecutionStatus::Succeeded);
    }

    #[test]
    fn finish_leaves_duration_none_when_finished_before_started() {
        let started = SystemTime::now();
        let finished = started - Duration::from_millis(50);

        let report =
            ExecutionReport::new("e".to_string(), "T".to_string(), "trace".to_string(), started)
                .finish(finished);

        assert_eq!(report.duration_ms, None);
        assert_eq!(report.finished_at, Some(finished));
    }

    #[test]
    fn adapter_error_conversions_preserve_message() {
        let err = AdapterError::WriteFailed("oops".to_string());

        let by_ref: AdapterReportError = (&err).into();
        assert_eq!(
            by_ref,
            AdapterReportError::WriteFailed {
                message: "oops".to_string()
            }
        );

        let owned: AdapterReportError = err.into();
        assert_eq!(
            owned,
            AdapterReportError::WriteFailed {
                message: "oops".to_string()
            }
        );

        let skipped: AdapterReportError = AdapterError::Skipped("done already".to_string()).into();
        assert_eq!(
            skipped,
            AdapterReportError::Skipped {
                message: "done already".to_string()
            }
        );
    }
}
