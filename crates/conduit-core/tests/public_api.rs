use conduit_core::execute_event;
use conduit_core::event::Event;
use conduit_core::adapter::sql::mapping::SqlMapping;
use conduit_core::adapter::document::mapping::DocumentMapping;

use std::collections::HashMap;
use std::env;

// ------------------------------------------------------------
// Helpers
// ------------------------------------------------------------

fn set_test_routing() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures");

    std::env::set_current_dir(dir).expect("failed to set test cwd");
}

// ------------------------------------------------------------
// Test
// ------------------------------------------------------------

#[test]
fn execute_event_runs_without_panic() {
    set_test_routing();

    let mut sql_mappings = HashMap::new();
    sql_mappings.insert(
        "UserCreated".to_string(),
        serde_yaml::from_str::<SqlMapping>(
            r#"
event: UserCreated
table: users
primary_key: id
columns:
  id: payload.id
"#,
        )
        .unwrap(),
    );

    let mut doc_mappings = HashMap::new();
    doc_mappings.insert(
        "UserCreated".to_string(),
        serde_yaml::from_str::<DocumentMapping>(
            r#"
event: UserCreated
collection: users
document:
  id: payload.id
"#,
        )
        .unwrap(),
    );

    let event = Event {
        event_id: "evt-1".into(),
        event_type: "UserCreated".into(),
        payload: r#"{ "id": "u1" }"#.into(),
        metadata: Default::default(),
    };

    let results = execute_event(sql_mappings, doc_mappings, event);

    assert!(!results.is_empty());
}
