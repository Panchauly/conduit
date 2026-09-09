//! Phase 14.5: applying sequences out of delivery order converges to the
//! state carried by the highest sequence — the KV adapter runs the exact
//! same `decide()` gate as SQL and document (Phase 12.2/13.1).

use conduit_core::adapter::StorageAdapter;
use conduit_core::adapter::keyvalue::mapping::KvMapping;
use conduit_core::adapter::keyvalue::runtime::KvRuntimeBuilder;
use conduit_core::adapter::keyvalue::store::KeyValueStore;
use conduit_core::event::Event;
use conduit_core::runtime::config::MigrationPolicy;
use conduit_core::upcast::UpcasterRegistry;

use std::collections::HashMap;
use std::sync::Arc;
use tempfile::tempdir;

fn user_view_updated(sequence: u64, state: &str) -> Event {
    Event {
        event_id: format!("evt-seq-{sequence}"),
        event_type: "UserViewUpdated".to_string(),
        payload: format!(r#"{{ "user_id": "u1", "state": "{state}" }}"#),
        metadata: HashMap::new(),
        version: 1,
        sequence,
    }
}

#[test]
fn kv_out_of_order_converges_to_highest_sequence() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempdir()?;
    let mut mappings = HashMap::new();
    mappings.insert(
        "UserViewUpdated".to_string(),
        serde_yaml::from_str::<KvMapping>(
            r#"
event: UserViewUpdated
namespace: user-view
version: 1
key: payload.user_id
on_existing: replace
value:
  state: payload.state
"#,
        )?,
    );

    let store = KeyValueStore::new(
        "kv-cache".to_string(),
        dir.path().to_path_buf(),
        10,
        KvRuntimeBuilder::new(mappings),
        Arc::new(UpcasterRegistry::new()),
        MigrationPolicy::default(),
    );

    // Delivery order 3, 1, 2 — not sorted.
    for (sequence, state) in [(3, "s3"), (1, "s1"), (2, "s2")] {
        let result = store.handle(&user_view_updated(sequence, state));
        assert!(result.is_success(), "seq {sequence}: {:?}", result.outcome);
    }

    let content = std::fs::read_to_string(dir.path().join("user-view").join("u1.json"))?;
    assert!(
        content.contains("\"s3\""),
        "final value must carry the state from the highest sequence: {content}"
    );

    Ok(())
}
