use std::collections::HashMap;

use crate::adapter::StorageAdapter;
use crate::adapter::document::file::FileDocumentAdapter;
use crate::adapter::document::mapping::DocumentMapping;
use crate::adapter::document::runtime::DocumentRuntimeBuilder;
use crate::adapter::sql::mapping::SqlMapping;
use crate::adapter::sql::runtime::SqlRuntimeBuilder;
use crate::adapter::sql::sqlite::SqliteAdapter;

use crate::runtime::config::{AdapterConfig, ConduitConfig};

pub fn build_adapters_from_config(
    config: &ConduitConfig,
    sql_mappings: HashMap<String, SqlMapping>,
    doc_mappings: HashMap<String, DocumentMapping>,
) -> Vec<Box<dyn StorageAdapter>> {
    let mut adapters: Vec<Box<dyn StorageAdapter>> = Vec::new();

    // Wrap mappings so they can be consumed exactly once
    let mut sql_mappings = Some(sql_mappings);
    let mut doc_mappings = Some(doc_mappings);

    for adapter in &config.adapters {
        match adapter {
            AdapterConfig::Sqlite(cfg) => {
                let mappings = sql_mappings.take().expect("sql mappings already consumed");

                let builder = SqlRuntimeBuilder::new(mappings);

                adapters.push(Box::new(SqliteAdapter::new(
                    cfg.id.clone(),
                    cfg.config.path.clone(),
                    cfg.priority,
                    builder,
                )));
            }

            AdapterConfig::File(cfg) => {
                let mappings = doc_mappings
                    .take()
                    .expect("document mappings already consumed");

                let builder = DocumentRuntimeBuilder::new(mappings);

                adapters.push(Box::new(FileDocumentAdapter::new(
                    cfg.id.clone(),
                    cfg.config.root.clone().into(),
                    cfg.priority,
                    builder,
                )));
            }
        }
    }

    adapters
}
