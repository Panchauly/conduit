use conduit_core::adapter::{AdapterError, AdapterResult, StorageAdapter};
use conduit_core::dispatch::dispatch;
use conduit_core::event::Event;
use conduit_core::routing::StorageKind;

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
    fn kind(&self) -> StorageKind {
        self.kind.clone()
    }

    fn id(&self) -> &str {
        self.id
    }

    fn priority(&self) -> u32 {
        10
    }

    fn handle(&self, _event: &Event) -> AdapterResult {
        if self.succeed {
            AdapterResult {
                adapter_id: self.id.to_string(),
                kind: self.kind.clone(),
                success: true,
                error: None,
            }
        } else {
            AdapterResult {
                adapter_id: self.id.to_string(),
                kind: self.kind.clone(),
                success: false,
                error: Some(AdapterError::WriteFailed("forced failure".into())),
            }
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
            id: "sql",
            kind: StorageKind::Sql,
            succeed: true,
        }),
        Box::new(TestAdapter {
            id: "doc",
            kind: StorageKind::Document,
            succeed: true,
        }),
    ];

    let results = dispatch(&event, &mut adapters);

    assert_eq!(results.len(), 2);
    assert!(results.iter().all(|r| r.success));
}

#[test]
fn dispatch_continues_on_adapter_failure() {
    set_test_routing();

    let event = test_event("FailingEvent");

    let mut adapters: Vec<Box<dyn StorageAdapter>> = vec![
        Box::new(TestAdapter {
            id: "sql",
            kind: StorageKind::Sql,
            succeed: false,
        }),
        Box::new(TestAdapter {
            id: "doc",
            kind: StorageKind::Document,
            succeed: true,
        }),
    ];

    let results = dispatch(&event, &mut adapters);

    // Dispatch must not panic
    // Dispatch must return results for executed adapters
    assert!(!results.is_empty());

    // All results must be well-formed
    for r in results {
        assert!(!r.adapter_id.is_empty());
    }
}

#[test]
fn dispatch_only_runs_routed_adapters() {
    set_test_routing();

    let event = test_event("UserCreated");

    let mut adapters: Vec<Box<dyn StorageAdapter>> = vec![
        Box::new(TestAdapter {
            id: "sql",
            kind: StorageKind::Sql,
            succeed: true,
        }),
        Box::new(TestAdapter {
            id: "doc",
            kind: StorageKind::Document,
            succeed: true,
        }),
        Box::new(TestAdapter {
            id: "kv",
            kind: StorageKind::KeyValue,
            succeed: true,
        }),
    ];

    let results = dispatch(&event, &mut adapters);

    // routing.json selects only Sql + Document
    assert_eq!(results.len(), 2);
    assert!(results.iter().all(|r| r.kind != StorageKind::KeyValue));
}
