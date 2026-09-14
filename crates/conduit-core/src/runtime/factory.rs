use std::collections::HashMap;
use std::sync::Arc;

use crate::adapter::StorageAdapter;
use crate::adapter::document::file::FileDocumentAdapter;
use crate::adapter::document::mapping::DocumentMapping;
use crate::adapter::document::runtime::DocumentRuntimeBuilder;
use crate::adapter::graph::mapping::GraphMapping;
use crate::adapter::graph::runtime::GraphRuntimeBuilder;
use crate::adapter::graph::store::GraphStore;
use crate::adapter::keyvalue::mapping::KvMapping;
use crate::adapter::keyvalue::redis::RedisAdapter;
use crate::adapter::keyvalue::runtime::KvRuntimeBuilder;
use crate::adapter::keyvalue::store::FileKvStore;
use crate::adapter::sql::mapping::SqlMapping;
use crate::adapter::sql::postgres::PostgresAdapter;
use crate::adapter::sql::runtime::SqlRuntimeBuilder;
use crate::adapter::sql::sqlite::SqliteAdapter;
use crate::upcast::UpcasterRegistry;

use crate::runtime::config::{AdapterConfig, ConduitConfig};

/// Build all adapters from config. Each SQL adapter gets a **clone** of `sql_mappings`; each file
/// adapter gets a **clone** of `document_mappings`; each key-value adapter gets a **clone** of
/// `kv_mappings` — so one event type can route to many targets (e.g. `pgsql_master`,
/// `pgsql_slave`) with the same projection definitions.
///
/// `upcasters` is shared (via `Arc`) across every adapter instance; `config.migration_policy`
/// governs how each adapter reacts when no upcaster chain exists to its mapping's target version.
#[allow(clippy::too_many_arguments)]
pub fn build_adapters_from_config(
    config: &ConduitConfig,
    sql_mappings: HashMap<String, SqlMapping>,
    document_mappings: HashMap<String, DocumentMapping>,
    kv_mappings: HashMap<String, KvMapping>,
    graph_mappings: HashMap<String, GraphMapping>,
    upcasters: Arc<UpcasterRegistry>,
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
                    Arc::clone(&upcasters),
                    config.migration_policy,
                )));
            }

            AdapterConfig::File(cfg) => {
                let builder = DocumentRuntimeBuilder::new(document_mappings.clone());
                adapters.push(Box::new(FileDocumentAdapter::new(
                    cfg.id.clone(),
                    cfg.config.root.clone().into(),
                    cfg.priority,
                    builder,
                    Arc::clone(&upcasters),
                    config.migration_policy,
                )));
            }

            AdapterConfig::KeyValue(cfg) => {
                let builder = KvRuntimeBuilder::new(kv_mappings.clone());
                adapters.push(Box::new(FileKvStore::new(
                    cfg.id.clone(),
                    cfg.config.root.clone().into(),
                    cfg.priority,
                    builder,
                    Arc::clone(&upcasters),
                    config.migration_policy,
                )));
            }

            AdapterConfig::Graph(cfg) => {
                let builder = GraphRuntimeBuilder::new(graph_mappings.clone());
                adapters.push(Box::new(GraphStore::new(
                    cfg.id.clone(),
                    cfg.config.root.clone().into(),
                    cfg.priority,
                    builder,
                    Arc::clone(&upcasters),
                    config.migration_policy,
                )));
            }

            AdapterConfig::Postgres(cfg) => {
                let builder = SqlRuntimeBuilder::new(sql_mappings.clone());
                let url = crate::runtime::config::expand_env(&cfg.config.url);
                adapters.push(Box::new(PostgresAdapter::new(
                    cfg.id.clone(),
                    &url,
                    cfg.config.pool_size.unwrap_or(4),
                    cfg.priority,
                    builder,
                    Arc::clone(&upcasters),
                    config.migration_policy,
                )));
            }

            AdapterConfig::Redis(cfg) => {
                let builder = KvRuntimeBuilder::new(kv_mappings.clone());
                let url = crate::runtime::config::expand_env(&cfg.config.url);
                adapters.push(Box::new(RedisAdapter::new(
                    cfg.id.clone(),
                    &url,
                    cfg.config.pool_size.unwrap_or(4),
                    cfg.priority,
                    builder,
                    Arc::clone(&upcasters),
                    config.migration_policy,
                )));
            }
        }
    }

    adapters.sort_by_key(|a| a.priority());
    adapters
}
