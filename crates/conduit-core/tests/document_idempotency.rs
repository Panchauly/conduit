use conduit_core::adapter::StorageAdapter;
use conduit_core::adapter::document::file::FileDocumentAdapter;
use conduit_core::adapter::document::mapping::DocumentMapping;
use conduit_core::adapter::document::runtime::DocumentRuntimeBuilder;
use conduit_core::event::Event;

use std::collections::HashMap;
use tempfile::tempdir;

// ------------------------------------------------------------
// Helpers
// ------------------------------------------------------------

fn test_event() -> Event {
    Event {
        event_id: "evt-1".to_string(),
        event_type: "UserCreated".to_string(),
        payload: r#"{ "id": "u1", "email": "a@b.com" }"#.to_string(),
        metadata: HashMap::new(),
    }
}

// ------------------------------------------------------------
// Test
// ------------------------------------------------------------

#[test]
fn document_adapter_is_idempotent() -> Result<(), Box<dyn std::error::Error>> {
    // Temp directory for document output
    let dir = tempdir()?;
    let root = dir.path();

    // Minimal document mapping
    let mut doc_mappings = HashMap::new();
    doc_mappings.insert(
        "UserCreated".to_string(),
        serde_yaml::from_str::<DocumentMapping>(
            r#"
event: UserCreated
collection: users
document:
  id: payload.id
  email: payload.email
"#,
        )?,
    );

    let builder = DocumentRuntimeBuilder::new(doc_mappings);

    let adapter = FileDocumentAdapter::new("file".to_string(), root.to_path_buf(), 10, builder);

    let event = test_event();

    // --------------------------------------------------
    // Call adapter DIRECTLY (no routing, no dispatch)
    // --------------------------------------------------

    let r1 = adapter.handle(&event);
    assert!(r1.success);

    let r2 = adapter.handle(&event);
    assert!(r2.success);

    // Verify document exists exactly once
    let doc_path = root.join("UserCreated").join("evt-1.json");

    assert!(doc_path.exists(), "document not written");

    // Verify idempotency guard exists
    let guard_path = root.join(".conduit").join("events").join("evt-1.done");

    assert!(guard_path.exists(), "idempotency guard missing");

    // Verify no duplicate documents
    let entries: Vec<_> = std::fs::read_dir(root.join("UserCreated"))?.collect();

    assert_eq!(entries.len(), 1, "duplicate document written");

    Ok(())
}
