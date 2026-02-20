use conduit_core::adapter::{AdapterError, AdapterResult, StorageAdapter};
use conduit_core::dispatch::dispatch;
use conduit_core::event::Event;
use conduit_core::execution::{AdapterOutcome, ExecutionStatus};
use conduit_core::routing::{StorageKind, route};

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

    let report = dispatch(&event, &mut adapters);

    assert_eq!(report.adapter_reports.len(), 2);
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

    let targets = route(&event);
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

    let report = dispatch(&event, &mut adapters);

    assert_eq!(report.adapter_reports.len(), 1);
    assert_eq!(report.adapter_reports[0].adapter_id, "sql-primary");
    assert_eq!(
        report.adapter_reports[0].outcome,
        AdapterOutcome::WriteFailed
    );
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

    let report = dispatch(&event, &mut adapters);

    // routing.json selects only Sql + Document
    assert_eq!(report.adapter_reports.len(), 2);
    assert!(
        report
            .adapter_reports
            .iter()
            .all(|r| r.storage_kind != StorageKind::KeyValue)
    );
}
