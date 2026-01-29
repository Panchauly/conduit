use conduit_core::event::Event;
use conduit_core::routing::{StorageKind, route};

use std::collections::HashMap;

fn set_test_routing() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures");

    std::env::set_current_dir(dir).expect("failed to set test cwd");
}

fn test_event(event_type: &str) -> Event {
    Event {
        event_id: "evt-1".to_string(),
        event_type: event_type.to_string(),
        payload: "{}".to_string(),
        metadata: HashMap::new(),
    }
}

#[test]
fn user_created_routes_to_sql_and_document() {
    set_test_routing();
    let event = test_event("UserCreated");

    let targets = route(&event);

    assert_eq!(
        targets,
        vec![StorageKind::Sql, StorageKind::Document],
        "UserCreated must route to SQL and Document"
    );
}

#[test]
fn cache_invalidated_routes_to_key_value() {
    set_test_routing();
    let event = test_event("CacheInvalidated");

    let targets = route(&event);

    assert_eq!(
        targets,
        vec![StorageKind::KeyValue],
        "CacheInvalidated must route to KeyValue"
    );
}

#[test]
fn unknown_event_routes_to_document_by_default() {
    set_test_routing();
    let event = test_event("UnknownEvent");

    let targets = route(&event);

    assert_eq!(
        targets,
        vec![StorageKind::Document],
        "Unknown events must route to Document by default"
    );
}
