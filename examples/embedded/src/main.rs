//! The "embedded crate" deployment mode (architecture.md §1.3): a host
//! service links `conduit-core` and calls `execute_event` directly — no CLI,
//! no config file, no gRPC. Run with `cargo run --manifest-path
//! examples/embedded/Cargo.toml` from the repo root.

use conduit_core::event::Event;
use conduit_core::execute_event;
use conduit_core::runtime::config::{
    AdapterConfig, ConduitConfig, RoutingConfig, SqliteAdapterConfig, SqliteConfig,
};
use std::collections::HashMap;

fn main() {
    let db_path = std::env::temp_dir().join("conduit-embedded-example.db");
    let _ = std::fs::remove_file(&db_path);
    rusqlite::Connection::open(&db_path)
        .unwrap()
        .execute("CREATE TABLE users (id TEXT PRIMARY KEY, name TEXT)", [])
        .unwrap();

    // `execute_event` (unlike `pipeline::run` / `run_sources`) routes through
    // a process-global routing table loaded once from `$ROUTING_CONFIG`
    // (default `./routing.json`) — there is no explicit-routing-map overload
    // for the single-call API. A host embedding conduit-core sets this once
    // at startup, same as any other process-wide config.
    let routing_path = std::env::temp_dir().join("conduit-embedded-example-routing.json");
    std::fs::write(&routing_path, r#"{"UserRegistered": ["sql-primary"]}"#).unwrap();
    // SAFETY: single-threaded example.
    unsafe {
        std::env::set_var("ROUTING_CONFIG", &routing_path);
    }

    let config = ConduitConfig {
        version: 1,
        routing: RoutingConfig {
            file: routing_path.to_string_lossy().into(),
        },
        adapters: vec![AdapterConfig::Sqlite(SqliteAdapterConfig {
            id: "sql-primary".into(),
            priority: 10,
            config: SqliteConfig {
                path: db_path.to_string_lossy().into(),
            },
            capabilities: None,
            depends_on: vec![],
        })],
        failure_policy: Default::default(),
        migration_policy: Default::default(),
        sources: Vec::new(),
    };

    let mut sql_mappings = HashMap::new();
    sql_mappings.insert(
        "UserRegistered".to_string(),
        serde_yaml::from_str(
            "event: UserRegistered\ntable: users\nprimary_key: id\nversion: 1\ncolumns:\n  id: payload.id\n  name: payload.name\n",
        )
        .unwrap(),
    );

    let event = Event {
        event_id: "e1".into(),
        event_type: "UserRegistered".into(),
        payload: r#"{"id":"u1","name":"Ada"}"#.into(),
        metadata: HashMap::new(),
        version: 1,
        sequence: 1,
    };

    let report = execute_event(
        &config,
        sql_mappings,
        HashMap::new(), // document_mappings
        HashMap::new(), // kv_mappings
        HashMap::new(), // graph_mappings
        event,
    );
    println!("status: {:?}", report.status);
    for a in &report.adapter_reports {
        println!("  {} -> {:?}", a.adapter_id, a.outcome);
    }

    let name: String = rusqlite::Connection::open(&db_path)
        .unwrap()
        .query_row("SELECT name FROM users WHERE id = 'u1'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(name, "Ada");
    println!("projected: users.u1.name = {name:?}");
}
