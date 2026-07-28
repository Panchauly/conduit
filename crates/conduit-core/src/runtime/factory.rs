use std::collections::HashMap;

use crate::adapter::StorageAdapter;
use crate::adapter::document::file::FileDocumentAdapter;
use crate::adapter::document::mapping::DocumentMapping;
use crate::adapter::document::runtime::DocumentRuntimeBuilder;
use crate::adapter::sql::mapping::SqlMapping;
use crate::adapter::sql::runtime::SqlRuntimeBuilder;
use crate::adapter::sql::sqlite::SqliteAdapter;

use crate::runtime::config::{AdapterConfig, ConduitConfig};

/// Build all adapters from config. Each SQL adapter gets a **clone** of `sql_mappings`; each file
/// adapter gets a **clone** of `document_mappings` — so one event type can route to many targets
/// (e.g. `pgsql_master`, `pgsql_slave`) with the same projection definitions.
pub fn build_adapters_from_config(
    config: &ConduitConfig,
    sql_mappings: HashMap<String, SqlMapping>,
    document_mappings: HashMap<String, DocumentMapping>,
) -> Vec<Box<dyn StorageAdapter>> {
    let mut adapters: Vec<Box<dyn StorageAdapter>> = Vec::new();

    for adapter in &config.adapters {
        match adapter {
            AdapterConfig::Sqlite(cfg) => {
                let builder = SqlRuntimeBuilder::new(sql_mappings.clone());
                adapters.push(Box::new(SqliteAdapter::new(
                    cfg.id.clone(),
                    cfg.config.path.clone(),
                    cfg.priority,
                    builder,
                )));
            }

            AdapterConfig::File(cfg) => {
                let builder = DocumentRuntimeBuilder::new(document_mappings.clone());
                adapters.push(Box::new(FileDocumentAdapter::new(
                    cfg.id.clone(),
                    cfg.config.root.clone().into(),
                    cfg.priority,
                    builder,
                )));
            }
        }
    }

    adapters.sort_by_key(|a| a.priority());
    adapters
}
