use conduit_core::adapter::document::mapping::DocumentMapping;
use conduit_core::adapter::sql::mapping::SqlMapping;
use conduit_core::event::Event;
use conduit_core::execute_event;
use conduit_core::runtime::config::{
    AdapterConfig, ConduitConfig, FailurePolicy, FileAdapterConfig, FileConfig, RoutingConfig,
    SqliteAdapterConfig, SqliteConfig,
};

use std::collections::HashMap;
use std::env;
use std::path::Path;

// ------------------------------------------------------------
// Helpers
// ------------------------------------------------------------

fn set_test_routing() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures");

    env::set_current_dir(dir).expect("failed to set test cwd");

    // IMPORTANT: routing.json MUST match adapter IDs in this test
    // routing.json content:
    // {
    //   "UserCreated": ["sqlite", "file"]
    // }
}

// ------------------------------------------------------------
// Test
// ------------------------------------------------------------

#[test]
fn execute_event_runs_without_panic() {
    set_test_routing();

    // -----------------------------
    // Config
    // -----------------------------
    let config = ConduitConfig {
        version: 1,
        routing: RoutingConfig {
            file: "routing.json".into(),
        },
        adapters: vec![
            AdapterConfig::Sqlite(SqliteAdapterConfig {
                id: "sql-primary".into(),
                priority: 10,
                config: SqliteConfig {
                    path: ":memory:".into(),
                },
                capabilities: None,
                depends_on: vec![],
            }),
            AdapterConfig::File(FileAdapterConfig {
                id: "doc-readmodel".into(),
                priority: 20,
                config: FileConfig {
                    root: "./target/test-docs".into(),
                },
                capabilities: None,
                depends_on: vec![],
            }),
        ],
        failure_policy: FailurePolicy::FailFast,
        migration_policy: Default::default(),
    };

    config.validate().expect("config must be valid");

    // -----------------------------
    // SQL mappings
    // -----------------------------
    let mut sql_mappings = HashMap::new();
    sql_mappings.insert(
        "UserCreated".to_string(),
        serde_yaml::from_str::<SqlMapping>(
            r#"
event: UserCreated
table: users
primary_key: id
version: 1
columns:
  id: payload.id
"#,
        )
        .unwrap(),
    );

    // -----------------------------
    // Document mappings
    // -----------------------------
    let mut doc_mappings = HashMap::new();
    doc_mappings.insert(
        "UserCreated".to_string(),
        serde_yaml::from_str::<DocumentMapping>(
            r#"
event: UserCreated
collection: users
version: 1
id: payload.id
document:
  id: payload.id
"#,
        )
        .unwrap(),
    );

    // -----------------------------
    // Event
    // -----------------------------
    let event = Event {
        event_id: "evt-1".into(),
        event_type: "UserCreated".into(),
        payload: r#"{ "id": "u1" }"#.into(),
        metadata: Default::default(),
        version: 1,
        sequence: 1,
    };

    // -----------------------------
    // Execute
    // -----------------------------
    let report = execute_event(&config, sql_mappings, doc_mappings, HashMap::new(), event);

    // -----------------------------
    // Assert
    // -----------------------------
    assert!(
        !report.adapter_reports.is_empty(),
        "UserCreated must execute at least one adapter"
    );

    for r in &report.adapter_reports {
        assert!(!r.adapter_id.is_empty(), "adapter_id must always be set");
    }
}
