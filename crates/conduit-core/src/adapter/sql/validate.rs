use std::collections::HashMap;

use crate::{adapter::sql::mapping::SqlMapping, routing::StorageKind};

pub fn validate_sql_mappings(
    routing: &HashMap<String, Vec<StorageKind>>,
    sql_mappings: &HashMap<String, SqlMapping>,
) -> Result<(), String> {
    // Rule A: routing → mapping
    for (event, targets) in routing {
        if targets.contains(&StorageKind::Sql) && !sql_mappings.contains_key(event) {
            return Err(format!(
                "Routing includes SQL for event '{}' but no SQL mapping found",
                event
            ));
        }
    }

    // Rule B + D: mapping → routing + PK presence
    for (event, mapping) in sql_mappings {
        match routing.get(event) {
            Some(targets) if targets.contains(&StorageKind::Sql) => {}
            Some(_) => {
                return Err(format!(
                    "SQL mapping exists for event '{}' but routing does not include SQL",
                    event
                ));
            }
            None => {
                return Err(format!(
                    "SQL mapping exists for event '{}' but event is not in routing.json",
                    event
                ));
            }
        }

        // Rule D: every primary key column (composite keys included) must exist in columns
        if let Some(missing) = mapping
            .primary_key
            .iter()
            .find(|c| !mapping.columns.contains_key(c.as_str()))
        {
            return Err(format!(
                "Primary key column '{}' not found in columns for event '{}'",
                missing, event
            ));
        }
    }

    Ok(())
}
