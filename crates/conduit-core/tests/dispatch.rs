use conduit_core::adapter::{AdapterError, AdapterResult, StorageAdapter};
use conduit_core::dispatch::dispatch;
use conduit_core::event::Event;
use conduit_core::execution::{AdapterOutcome, ExecutionStatus};
use conduit_core::routing::{route, AdapterId, StorageKind};
use conduit_core::runtime::config::FailurePolicy;
use conduit_core::AdapterExecutionMeta;

use std::collections::HashMap;

// ------------------------------------------------------------
// Test Adapter
// ------------------------------------------------------------

struct TestAdapter {
    id: &'static str,
    kind: StorageKind,
    succeed: bool,
}

impl StorageAdapter for TestAdapter {
    fn id(&self) -> &str {
        self.id
    }

    fn kind(&self) -> StorageKind {
        self.kind
    }

    fn priority(&self) -> u32 {
        0
    }

    fn handle(&self, _event: &Event) -> AdapterResult {
        AdapterResult {
            adapter_id: self.id.to_string(), // 🔴 THIS MUST MATCH routing.json
            kind: self.kind,
            success: self.succeed,
            error: if self.succeed {
                None
            } else {
                Some(AdapterError::WriteFailed("forced failure".to_string()))
            },
        }
    }
}

// ------------------------------------------------------------
// Helpers
// ------------------------------------------------------------

fn set_test_routing() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures");

    std::env::set_current_dir(dir).expect("failed to set test cwd");
}

fn test_event(event_type: &str) -> Event {
    Event {
        event_id: "evt-1".into(),
        event_type: event_type.into(),
        payload: "{}".into(),
        metadata: HashMap::new(),
    }
}

/// Matches typical config: SQL before document (priorities 10 / 20).
fn fixture_adapter_meta() -> HashMap<AdapterId, AdapterExecutionMeta> {
    HashMap::from([
        (
            "sql-primary".into(),
            AdapterExecutionMeta {
                priority: 10,
                depends_on: vec![],
            },
        ),
        (
            "doc-readmodel".into(),
            AdapterExecutionMeta {
                priority: 20,
                depends_on: vec![],
            },
        ),
        (
            "kv-cache".into(),
            AdapterExecutionMeta {
                priority: 5,
                depends_on: vec![],
            },
        ),
    ])
}

// ------------------------------------------------------------
// Tests
// ------------------------------------------------------------

#[test]
fn dispatch_executes_all_targeted_adapters() {
    set_test_routing();

    let event = test_event("UserCreated");

    let mut adapters: Vec<Box<dyn StorageAdapter>> = vec![
        Box::new(TestAdapter {
            id: "sql-primary",
            kind: StorageKind::Sql,
            succeed: true,
        }),
        Box::new(TestAdapter {
            id: "doc-readmodel",
            kind: StorageKind::Document,
            succeed: true,
        }),
    ];

    let meta = fixture_adapter_meta();
    let report = dispatch(
        &event,
        &mut adapters,
        FailurePolicy::FailFast,
        &meta,
    );

    assert_eq!(report.adapter_reports.len(), 2);
    assert_eq!(report.adapter_reports[0].adapter_id, "sql-primary");
    assert_eq!(report.adapter_reports[1].adapter_id, "doc-readmodel");
    assert_eq!(report.status, ExecutionStatus::Succeeded);
    assert!(
        report
            .adapter_reports
            .iter()
            .all(|r| r.outcome == AdapterOutcome::Succeeded)
    );
}

#[test]
fn dispatch_stops_on_adapter_failure() {
    set_test_routing();

    let event = test_event("FailingEvent");

    let targets = route(&event).expect("routing table loads from test fixtures");
    assert!(!targets.is_empty(), "FailingEvent must be routed");

    let mut adapters: Vec<Box<dyn StorageAdapter>> = vec![
        Box::new(TestAdapter {
            id: "sql-primary",
            kind: StorageKind::Sql,
            succeed: false,
        }),
        Box::new(TestAdapter {
            id: "doc-readmodel",
            kind: StorageKind::Document,
            succeed: true,
        }),
    ];

    let meta = fixture_adapter_meta();
    let report = dispatch(
        &event,
        &mut adapters,
        FailurePolicy::FailFast,
        &meta,
    );

    assert_eq!(report.adapter_reports.len(), 1);
    assert_eq!(report.adapter_reports[0].adapter_id, "sql-primary");
    assert_eq!(
        report.adapter_reports[0].outcome,
        AdapterOutcome::WriteFailed
    );
    assert_eq!(report.status, ExecutionStatus::Failed);
}

#[test]
fn dispatch_continue_on_error_runs_all_adapters() {
    set_test_routing();

    let event = test_event("UserCreated");

    let mut adapters: Vec<Box<dyn StorageAdapter>> = vec![
        Box::new(TestAdapter {
            id: "sql-primary",
            kind: StorageKind::Sql,
            succeed: false,
        }),
        Box::new(TestAdapter {
            id: "doc-readmodel",
            kind: StorageKind::Document,
            succeed: true,
        }),
    ];

    let meta = fixture_adapter_meta();
    let report = dispatch(
        &event,
        &mut adapters,
        FailurePolicy::ContinueOnError,
        &meta,
    );

    assert_eq!(report.adapter_reports.len(), 2);
    assert_eq!(report.adapter_reports[0].outcome, AdapterOutcome::WriteFailed);
    assert_eq!(report.adapter_reports[1].outcome, AdapterOutcome::Succeeded);
    assert_eq!(report.status, ExecutionStatus::Failed);
}

#[test]
fn dispatch_only_runs_routed_adapters() {
    set_test_routing();

    let event = test_event("UserCreated");

    let mut adapters: Vec<Box<dyn StorageAdapter>> = vec![
        Box::new(TestAdapter {
            id: "sql-primary",
            kind: StorageKind::Sql,
            succeed: true,
        }),
        Box::new(TestAdapter {
            id: "doc-readmodel",
            kind: StorageKind::Document,
            succeed: true,
        }),
        Box::new(TestAdapter {
            id: "kv-cache",
            kind: StorageKind::KeyValue,
            succeed: true,
        }),
    ];

    let meta = fixture_adapter_meta();
    let report = dispatch(
        &event,
        &mut adapters,
        FailurePolicy::FailFast,
        &meta,
    );

    // routing.json selects only Sql + Document
    assert_eq!(report.adapter_reports.len(), 2);
    assert!(
        report
            .adapter_reports
            .iter()
            .all(|r| r.storage_kind != StorageKind::KeyValue)
    );
}

#[test]
fn dispatch_runs_dependency_before_dependent() {
    set_test_routing();
    let event = test_event("UserCreated");
    let mut adapters: Vec<Box<dyn StorageAdapter>> = vec![
        Box::new(TestAdapter {
            id: "sql-primary",
            kind: StorageKind::Sql,
            succeed: true,
        }),
        Box::new(TestAdapter {
            id: "doc-readmodel",
            kind: StorageKind::Document,
            succeed: true,
        }),
    ];
    let mut meta = fixture_adapter_meta();
    meta.insert(
        "doc-readmodel".into(),
        AdapterExecutionMeta {
            priority: 5,
            depends_on: vec!["sql-primary".into()],
        },
    );
    let report = dispatch(
        &event,
        &mut adapters,
        FailurePolicy::FailFast,
        &meta,
    );
    assert_eq!(report.adapter_reports[0].adapter_id, "sql-primary");
    assert_eq!(report.adapter_reports[1].adapter_id, "doc-readmodel");
}

#[test]
fn dispatch_cycle_yields_failed_report_not_panic() {
    set_test_routing();
    let event = test_event("UserCreated");
    let mut adapters: Vec<Box<dyn StorageAdapter>> = vec![
        Box::new(TestAdapter {
            id: "sql-primary",
            kind: StorageKind::Sql,
            succeed: true,
        }),
        Box::new(TestAdapter {
            id: "doc-readmodel",
            kind: StorageKind::Document,
            succeed: true,
        }),
    ];
    let mut meta = fixture_adapter_meta();
    meta.insert(
        "sql-primary".into(),
        AdapterExecutionMeta {
            priority: 10,
            depends_on: vec!["doc-readmodel".into()],
        },
    );
    meta.insert(
        "doc-readmodel".into(),
        AdapterExecutionMeta {
            priority: 20,
            depends_on: vec!["sql-primary".into()],
        },
    );
    let report = dispatch(
        &event,
        &mut adapters,
        FailurePolicy::FailFast,
        &meta,
    );
    assert_eq!(report.status, ExecutionStatus::Failed);
    assert_eq!(report.adapter_reports.len(), 1);
    assert_eq!(report.adapter_reports[0].adapter_id, "_dispatch");
    assert!(report.adapter_reports[0]
        .error
        .as_ref()
        .is_some_and(|e| format!("{:?}", e).contains("cyclic")));
}
