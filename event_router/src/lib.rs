#[derive(Debug, PartialEq)]
pub enum StorageKind {
    Sql,
    Document,
    KeyValue,
    Graph,
}

#[derive(Debug)]
pub struct Event {
    pub event_type: String,
}

pub fn route(event: &Event) -> Vec<StorageKind> {
    match event.event_type.as_str() {
        "UserCreated" => vec![StorageKind::Sql, StorageKind::Document],
        "CacheInvalidated" => vec![StorageKind::KeyValue],
        _ => vec![StorageKind::Document],
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_created_routes_to_sql_and_document() {
        let event = Event {
            event_type: "UserCreated".to_string(),
        };

        let targets = route(&event);

        assert_eq!(
            targets,
            vec![StorageKind::Sql, StorageKind::Document]
        );
    }

    #[test]
    fn cache_invalidated_routes_to_kv() {
        let event = Event {
            event_type: "CacheInvalidated".to_string(),
        };

        let targets = route(&event);

        assert_eq!(
            targets,
            vec![StorageKind::KeyValue]
        );
    }

    #[test]
    fn unknown_event_routes_to_document_by_default() {
        let event = Event {
            event_type: "Unknown".to_string(),
        };

        let targets = route(&event);

        assert_eq!(
            targets,
            vec![StorageKind::Document]
        );
    }
}

