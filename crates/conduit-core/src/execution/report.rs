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
