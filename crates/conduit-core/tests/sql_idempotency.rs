use conduit_core::adapter::StorageAdapter;
use conduit_core::adapter::sql::runtime::SqlRuntimeBuilder;
use conduit_core::adapter::sql::sqlite::SqliteAdapter;
use conduit_core::event::Event;
use conduit_core::runtime::config::MigrationPolicy;
use conduit_core::upcast::UpcasterRegistry;

use rusqlite::Connection;
use std::collections::HashMap;
use std::sync::Arc;
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
        version: 1,
    }
}

// ------------------------------------------------------------
// Test
// ------------------------------------------------------------

#[test]
fn sql_adapter_is_idempotent() -> Result<(), Box<dyn std::error::Error>> {
    // Temp directory for SQLite DB
    let dir = tempdir()?;
    let db_path = dir.path().join("test.db");

    // Create schema (explicit, no magic)
    {
        let conn = Connection::open(&db_path)?;
        conn.execute(
            "CREATE TABLE users (
                id TEXT PRIMARY KEY,
                email TEXT
            )",
            [],
        )?;
        conn.execute(
            "CREATE TABLE conduit_events (
                event_id TEXT PRIMARY KEY,
                processed_at TEXT NOT NULL
            )",
            [],
        )?;
    }

    // Minimal SQL mapping
    let mut sql_mappings = HashMap::new();
    sql_mappings.insert(
        "UserCreated".to_string(),
        serde_yaml::from_str(
            r#"
event: UserCreated
table: users
primary_key: id
version: 1
columns:
  id: payload.id
  email: payload.email
"#,
        )?,
    );

    let builder = SqlRuntimeBuilder::new(sql_mappings);

    let sql_adapter = SqliteAdapter::new(
        "sqlite".to_string(),
        db_path.to_string_lossy().to_string(),
        10,
        builder,
        Arc::new(UpcasterRegistry::new()),
        MigrationPolicy::default(),
    );

    let event = test_event();

    // --------------------------------------------------
    // Call adapter DIRECTLY (no routing, no dispatch)
    // --------------------------------------------------

    let r1 = sql_adapter.handle(&event);
    assert!(r1.success);

    let r2 = sql_adapter.handle(&event);
    assert!(r2.success);

    // Verify only one row exists
    let conn = Connection::open(&db_path)?;
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))?;

    assert_eq!(count, 1, "duplicate row inserted");

    Ok(())
}
