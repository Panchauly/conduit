//! Phase 12.6: replaying `[create s1, update s2, update s3]` from an empty
//! database produces the same final state as dispatching the same three
//! events one at a time in real time — extends `replay_versioned_stream.rs`
//! to `on_existing: replace`.

use conduit_core::adapter::sql::mapping::SqlMapping;
use conduit_core::event::Event;
use conduit_core::execute_event;
use conduit_core::replay::{ReplayContext, events_from_path};
use conduit_core::runtime::config::{
    AdapterConfig, ConduitConfig, RoutingConfig, SqliteAdapterConfig, SqliteConfig,
};

use rusqlite::Connection;
use std::collections::HashMap;
use std::fs;
use tempfile::tempdir;

fn sql_mapping_replace() -> SqlMapping {
    serde_yaml::from_str(
        r#"
event: UserUpdated
table: users
primary_key: id
version: 1
on_existing: replace
columns:
  id: payload.id
  name: payload.name
"#,
    )
    .unwrap()
}

fn init_users_table(db_path: &std::path::Path) {
    let conn = Connection::open(db_path).unwrap();
    conn.execute("CREATE TABLE users (id TEXT PRIMARY KEY, name TEXT)", [])
        .unwrap();
}

/// [create s1, rename s2, rename s3] — same event type/mapping throughout;
/// `on_existing: replace` means later deliveries overwrite the full row.
fn create_then_update_events() -> Vec<Event> {
    vec![
        Event {
            event_id: "e1".to_string(),
            event_type: "UserUpdated".to_string(),
            payload: r#"{"id":"u1","name":"Alice"}"#.to_string(),
            metadata: HashMap::new(),
            version: 1,
            sequence: 1,
        },
        Event {
            event_id: "e2".to_string(),
            event_type: "UserUpdated".to_string(),
            payload: r#"{"id":"u1","name":"Alice R1"}"#.to_string(),
            metadata: HashMap::new(),
            version: 1,
            sequence: 2,
        },
        Event {
            event_id: "e3".to_string(),
            event_type: "UserUpdated".to_string(),
            payload: r#"{"id":"u1","name":"Alice R2"}"#.to_string(),
            metadata: HashMap::new(),
            version: 1,
            sequence: 3,
        },
    ]
}

fn read_user(db_path: &std::path::Path) -> (String, i64) {
    let conn = Connection::open(db_path).unwrap();
    let name: String = conn
        .query_row("SELECT name FROM users WHERE id = 'u1'", [], |r| r.get(0))
        .unwrap();
    let last_sequence: i64 = conn
        .query_row(
            "SELECT last_sequence FROM conduit_projection_state WHERE target_table = 'users'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    (name, last_sequence)
}

#[test]
fn replay_and_real_time_dispatch_agree_on_final_state() {
    let tmp = tempdir().unwrap();

    // -----------------------------------------------------------------
    // Part 1: replay a directory of [create, update, update] event files.
    // -----------------------------------------------------------------
    let replay_db = tmp.path().join("replay.db");
    init_users_table(&replay_db);

    let events_dir = tmp.path().join("events");
    fs::create_dir_all(&events_dir).unwrap();
    for (i, event) in create_then_update_events().into_iter().enumerate() {
        fs::write(
            events_dir.join(format!("{:02}.json", i)),
            serde_json::to_string(&event).unwrap(),
        )
        .unwrap();
    }

    let mut sql_mappings = HashMap::new();
    sql_mappings.insert("UserUpdated".to_string(), sql_mapping_replace());

    let mut routing_rules = HashMap::new();
    routing_rules.insert("UserUpdated".to_string(), vec!["sql-primary".to_string()]);

    let replay_config = ConduitConfig {
        version: 1,
        routing: RoutingConfig {
            file: "routing.json".into(),
        },
        adapters: vec![AdapterConfig::Sqlite(SqliteAdapterConfig {
            id: "sql-primary".into(),
            priority: 10,
            config: SqliteConfig {
                path: replay_db.to_string_lossy().into(),
            },
            capabilities: Some(vec![
                conduit_core::runtime::config::AdapterCapability::Write,
                conduit_core::runtime::config::AdapterCapability::Upsert,
            ]),
            depends_on: vec![],
        })],
        failure_policy: Default::default(),
        migration_policy: Default::default(),
    };
    replay_config.validate().unwrap();

    let mut ctx = ReplayContext::new(
        &replay_config,
        routing_rules.clone(),
        sql_mappings.clone(),
        HashMap::new(),
        HashMap::new(),
        HashMap::new(),
    );
    let mut iter = events_from_path(&events_dir).unwrap();
    let report = ctx.run_stream(&mut iter).unwrap();

    assert_eq!(report.events_processed, 3, "{:?}", report.per_event);
    assert_eq!(report.events_succeeded, 3);
    assert_eq!(report.events_failed, 0);

    let replay_state = read_user(&replay_db);
    assert_eq!(replay_state, ("Alice R2".to_string(), 3));

    // -----------------------------------------------------------------
    // Part 2: dispatch the same three events one at a time, in order.
    // -----------------------------------------------------------------
    let live_db = tmp.path().join("live.db");
    init_users_table(&live_db);

    let live_config = ConduitConfig {
        version: 1,
        routing: RoutingConfig {
            file: "routing.json".into(),
        },
        adapters: vec![AdapterConfig::Sqlite(SqliteAdapterConfig {
            id: "sql-primary".into(),
            priority: 10,
            config: SqliteConfig {
                path: live_db.to_string_lossy().into(),
            },
            capabilities: Some(vec![
                conduit_core::runtime::config::AdapterCapability::Write,
                conduit_core::runtime::config::AdapterCapability::Upsert,
            ]),
            depends_on: vec![],
        })],
        failure_policy: Default::default(),
        migration_policy: Default::default(),
    };
    live_config.validate().unwrap();

    let routing_json = tmp.path().join("routing.json");
    fs::write(&routing_json, r#"{"UserUpdated": ["sql-primary"]}"#).unwrap();
    // SAFETY: single-threaded within this test; no other test in this binary
    // reads/writes ROUTING_CONFIG or the global routing table.
    unsafe {
        std::env::set_var("ROUTING_CONFIG", &routing_json);
    }

    for event in create_then_update_events() {
        let report = execute_event(
            &live_config,
            sql_mappings.clone(),
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            event,
        );
        assert_eq!(
            report.status,
            conduit_core::execution::ExecutionStatus::Succeeded,
            "{:?}",
            report
        );
    }

    let live_state = read_user(&live_db);
    assert_eq!(live_state, ("Alice R2".to_string(), 3));

    // -----------------------------------------------------------------
    // Both pipelines must converge on the exact same final state.
    // -----------------------------------------------------------------
    assert_eq!(replay_state, live_state);
}
