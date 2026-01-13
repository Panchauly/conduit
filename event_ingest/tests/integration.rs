#[cfg(test)]
mod tests {

    use std::collections::HashMap;
    use std::path::PathBuf;

    use event_ingest::adapter::StorageAdapter;
    use event_ingest::adapter::document::adapter::DocumentAdapter;
    use event_ingest::adapter::document::loader::load_document_mappings;
    use event_ingest::adapter::sql::SqlAdapter;
    use event_ingest::adapter::sql::loader::load_sql_mappings;
    use event_ingest::adapter::sql::mapping::SqlMapping;
    use event_ingest::dispatch::dispatch;
    use event_ingest::event::Event;
    use event_ingest::routing::{StorageKind, route};

    #[test]
    fn user_created_routes_to_sql_and_document() {
        let event = Event {
            event_type: "UserCreated".to_string(),
            payload: "{}".to_string(),
            metadata: HashMap::new(),
        };

        let targets = route(&event);

        assert_eq!(targets, vec![StorageKind::Sql, StorageKind::Document]);
    }

    #[test]
    fn cache_invalidated_routes_to_kv() {
        let event = Event {
            event_type: "CacheInvalidated".to_string(),
            payload: "{}".to_string(),
            metadata: HashMap::new(),
        };

        let targets = route(&event);

        assert_eq!(targets, vec![StorageKind::KeyValue]);
    }

    #[test]
    fn unknown_event_routes_to_document_by_default() {
        let event = Event {
            event_type: "Unknown".to_string(),
            payload: "{}".to_string(),
            metadata: HashMap::new(),
        };

        let targets = route(&event);

        assert_eq!(targets, vec![StorageKind::Document]);
    }

    #[test]
    fn dispatch_calls_correct_adapters() -> Result<(), Box<dyn std::error::Error>> {
        let event = Event {
            event_type: "UserCreated".to_string(),
            payload: r#"{ "id": "u1", "email": "a@b.com" }"#.to_string(),
            metadata: std::collections::HashMap::new(),
        };

        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let doc_mappings = load_document_mappings(base.join("mappings").join("document"))?;
        let sql_mappings = load_sql_mappings(base.join("mappings").join("sql"))?;

        let doc_adapter = DocumentAdapter::new(doc_mappings);
        let sql_adapter = SqlAdapter::new(sql_mappings);

        let adapters: Vec<Box<dyn StorageAdapter>> =
            vec![Box::new(sql_adapter), Box::new(doc_adapter)];

        let results = dispatch(&event, &adapters);

        assert!(
            results
                .iter()
                .any(|(k, r)| *k == StorageKind::Sql && r.is_ok())
        );
        assert!(
            results
                .iter()
                .any(|(k, r)| *k == StorageKind::Document && r.is_ok())
        );

        Ok(())
    }

    #[test]
    fn sql_mapping_deserializes() {
        let yaml = r#"
event: UserCreated
table: users
primary_key: id
columns:
  id: payload.id
  email: payload.email
foreign_keys:
  user_id: users.id
"#;

        let mapping: SqlMapping = serde_yaml::from_str(yaml).unwrap();

        assert_eq!(mapping.event, "UserCreated");
        assert_eq!(mapping.table, "users");
        assert_eq!(mapping.primary_key, "id");
        assert!(mapping.columns.contains_key("email"));
    }

    #[test]
    fn loads_sql_mappings_from_directory() {
        let dir = "tests/fixtures/sql";

        let mappings = load_sql_mappings(dir).unwrap();

        assert!(mappings.contains_key("UserCreated"));
        assert!(mappings.contains_key("OrderPlaced"));
    }
}
