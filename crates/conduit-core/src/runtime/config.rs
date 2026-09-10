use core::fmt;
use serde::Deserialize;
use std::collections::HashSet;

use crate::routing::AdapterId;

// ------------------------------------------------------------
// Failure Policy (Phase 6.2)
// ------------------------------------------------------------

/// When an adapter fails, stop immediately (default) or run all routed adapters and report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailurePolicy {
    /// Stop on first adapter failure; report includes only adapters run so far.
    #[default]
    FailFast,

    /// Run all routed adapters; report includes every adapter outcome.
    ContinueOnError,
}

// ------------------------------------------------------------
// Migration Policy (Phase 10.3)
// ------------------------------------------------------------

/// Governs how a version-mismatched event (no upcaster chain to a mapping's
/// target version) is handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MigrationPolicy {
    /// Fail the adapter immediately when no upcaster chain exists (default).
    #[default]
    Strict,

    /// Skip projection for un-upcastable events; batch continues.
    IgnoreUnmatched,
}

// ------------------------------------------------------------
// Adapter Capabilities (Phase 6.4)
// ------------------------------------------------------------

/// Strongly-typed adapter capability declarations.
/// Used for mapping compatibility validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdapterCapability {
    /// Adapter can write data (baseline requirement).
    Write,

    /// Adapter guarantees idempotent writes.
    Idempotent,

    /// Adapter supports upsert semantics.
    Upsert,

    /// Adapter supports transactional boundaries.
    Transactions,

    /// Adapter supports removing a projected entity (Phase 13).
    Delete,
}

/// Declared capabilities of an adapter.
/// Optional and forward-compatible.
pub type AdapterCapabilities = Vec<AdapterCapability>;

// ------------------------------------------------------------
// Root Config
// ------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct ConduitConfig {
    pub version: u32,
    pub routing: RoutingConfig,
    pub adapters: Vec<AdapterConfig>,

    /// When an adapter fails: fail_fast (default) or continue_on_error.
    #[serde(default)]
    pub failure_policy: FailurePolicy,

    /// How version-mismatched events are handled: strict (default) or ignore_unmatched.
    #[serde(default)]
    pub migration_policy: MigrationPolicy,

    /// Phase 17: configured event sources. Empty (the default) → `conduit run`
    /// still works with an explicit `--event <file>`; the continuous source
    /// loop requires at least one.
    #[serde(default)]
    pub sources: Vec<SourceConfig>,
}

impl ConduitConfig {
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.version != 1 {
            return Err(ConfigError::UnsupportedVersion(self.version));
        }

        if self.adapters.is_empty() {
            return Err(ConfigError::NoAdaptersDefined);
        }

        let mut ids = HashSet::new();

        for adapter in &self.adapters {
            let id = adapter.id();
            if !ids.insert(id.to_string()) {
                return Err(ConfigError::DuplicateAdapterId(id.to_string()));
            }
        }

        for adapter in &self.adapters {
            let id = adapter.id();
            for dep in adapter.depends_on() {
                if dep == id {
                    return Err(ConfigError::SelfDependency(id.to_string()));
                }
                if !ids.contains(dep.as_str()) {
                    return Err(ConfigError::UnknownDependency {
                        adapter: id.to_string(),
                        dependency: dep.clone(),
                    });
                }
            }
        }

        // Phase 17: source ids are unique (and distinct from nothing else —
        // routing is by event_type, source_id is observability only).
        let mut source_ids = HashSet::new();
        for source in &self.sources {
            if !source_ids.insert(source.id().to_string()) {
                return Err(ConfigError::DuplicateSourceId(source.id().to_string()));
            }
        }

        Ok(())
    }
}

// ------------------------------------------------------------
// Routing
// ------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct RoutingConfig {
    pub file: String,
}

// ------------------------------------------------------------
// Adapter Configs
// ------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum AdapterConfig {
    #[serde(rename = "sqlite")]
    Sqlite(SqliteAdapterConfig),

    #[serde(rename = "file")]
    File(FileAdapterConfig),

    #[serde(rename = "keyvalue")]
    KeyValue(KeyValueAdapterConfig),

    #[serde(rename = "graph")]
    Graph(GraphAdapterConfig),
}

#[derive(Debug, Deserialize)]
pub struct SqliteAdapterConfig {
    pub id: String,
    pub priority: u32,
    pub config: SqliteConfig,

    /// Optional declared capabilities (Phase 6.4).
    #[serde(default)]
    pub capabilities: Option<AdapterCapabilities>,

    /// Adapters that must run before this one (Phase 9); must be on the same route for each event.
    #[serde(default)]
    pub depends_on: Vec<AdapterId>,
}

#[derive(Debug, Deserialize)]
pub struct SqliteConfig {
    pub path: String,
}

#[derive(Debug, Deserialize)]
pub struct FileAdapterConfig {
    pub id: String,
    pub priority: u32,
    pub config: FileConfig,

    /// Optional declared capabilities (Phase 6.4).
    #[serde(default)]
    pub capabilities: Option<AdapterCapabilities>,

    /// Adapters that must run before this one (Phase 9); must be on the same route for each event.
    #[serde(default)]
    pub depends_on: Vec<AdapterId>,
}

#[derive(Debug, Deserialize)]
pub struct FileConfig {
    pub root: String,
}

#[derive(Debug, Deserialize)]
pub struct KeyValueAdapterConfig {
    pub id: String,
    pub priority: u32,
    pub config: KeyValueConfig,

    /// Optional declared capabilities (Phase 6.4).
    #[serde(default)]
    pub capabilities: Option<AdapterCapabilities>,

    /// Adapters that must run before this one (Phase 9); must be on the same route for each event.
    #[serde(default)]
    pub depends_on: Vec<AdapterId>,
}

#[derive(Debug, Deserialize)]
pub struct KeyValueConfig {
    pub root: String,
}

#[derive(Debug, Deserialize)]
pub struct GraphAdapterConfig {
    pub id: String,
    pub priority: u32,
    pub config: GraphConfig,

    /// Optional declared capabilities (Phase 6.4).
    #[serde(default)]
    pub capabilities: Option<AdapterCapabilities>,

    /// Adapters that must run before this one (Phase 9); must be on the same route for each event.
    #[serde(default)]
    pub depends_on: Vec<AdapterId>,
}

#[derive(Debug, Deserialize)]
pub struct GraphConfig {
    pub root: String,
}

// ------------------------------------------------------------
// Source Configs (Phase 17)
// ------------------------------------------------------------

/// A configured event source — the input-side mirror of [`AdapterConfig`].
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SourceConfig {
    /// Watches a directory for event files (`*.json` / `*.ndjson` / `*.jsonl`).
    Directory(DirectorySourceConfig),
    /// NDJSON on standard input, one event per line.
    Stdin(StdinSourceConfig),
}

#[derive(Debug, Deserialize)]
pub struct DirectorySourceConfig {
    pub id: String,
    /// Directory of event files, relative to the config file's directory.
    pub path: String,
    /// Where the checkpoint sidecar lives, relative to the config file's directory.
    pub state_dir: String,
}

#[derive(Debug, Deserialize)]
pub struct StdinSourceConfig {
    pub id: String,
    pub state_dir: String,
}

impl SourceConfig {
    pub fn id(&self) -> &str {
        match self {
            SourceConfig::Directory(c) => &c.id,
            SourceConfig::Stdin(c) => &c.id,
        }
    }
}

// ------------------------------------------------------------
// Errors
// ------------------------------------------------------------

#[derive(Debug)]
pub enum ConfigError {
    UnsupportedVersion(u32),
    NoAdaptersDefined,
    DuplicateAdapterId(String),
    DuplicateSourceId(String),
    SelfDependency(String),
    UnknownDependency { adapter: String, dependency: String },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::UnsupportedVersion(v) => {
                write!(f, "unsupported config version: {}", v)
            }
            ConfigError::NoAdaptersDefined => {
                write!(f, "no adapters defined in config")
            }
            ConfigError::DuplicateAdapterId(id) => {
                write!(f, "duplicate adapter id: {}", id)
            }
            ConfigError::DuplicateSourceId(id) => {
                write!(f, "duplicate source id: {}", id)
            }
            ConfigError::SelfDependency(id) => {
                write!(f, "adapter {:?} cannot depend on itself", id)
            }
            ConfigError::UnknownDependency {
                adapter,
                dependency,
            } => {
                write!(
                    f,
                    "adapter {:?} depends on unknown adapter {:?}",
                    adapter, dependency
                )
            }
        }
    }
}

impl std::error::Error for ConfigError {}

// ------------------------------------------------------------
// AdapterConfig Helpers
// ------------------------------------------------------------

impl AdapterConfig {
    pub fn id(&self) -> &str {
        match self {
            AdapterConfig::Sqlite(cfg) => &cfg.id,
            AdapterConfig::File(cfg) => &cfg.id,
            AdapterConfig::KeyValue(cfg) => &cfg.id,
            AdapterConfig::Graph(cfg) => &cfg.id,
        }
    }

    pub fn priority(&self) -> u32 {
        match self {
            AdapterConfig::Sqlite(cfg) => cfg.priority,
            AdapterConfig::File(cfg) => cfg.priority,
            AdapterConfig::KeyValue(cfg) => cfg.priority,
            AdapterConfig::Graph(cfg) => cfg.priority,
        }
    }

    /// Declared capabilities for this adapter (if any).
    pub fn capabilities(&self) -> Option<&[AdapterCapability]> {
        match self {
            AdapterConfig::Sqlite(cfg) => cfg.capabilities.as_deref(),
            AdapterConfig::File(cfg) => cfg.capabilities.as_deref(),
            AdapterConfig::KeyValue(cfg) => cfg.capabilities.as_deref(),
            AdapterConfig::Graph(cfg) => cfg.capabilities.as_deref(),
        }
    }

    pub fn depends_on(&self) -> &[AdapterId] {
        match self {
            AdapterConfig::Sqlite(cfg) => &cfg.depends_on,
            AdapterConfig::File(cfg) => &cfg.depends_on,
            AdapterConfig::KeyValue(cfg) => &cfg.depends_on,
            AdapterConfig::Graph(cfg) => &cfg.depends_on,
        }
    }
}

#[cfg(test)]
mod source_tests {
    use super::*;

    #[test]
    fn parses_directory_and_stdin_sources() {
        let yaml = r#"
version: 1
routing:
  file: routing.json
adapters:
  - type: sqlite
    id: sql-primary
    priority: 10
    config: { path: ":memory:" }
sources:
  - type: directory
    id: inbox
    path: ./events
    state_dir: ./.conduit/sources
  - type: stdin
    id: pipe
    state_dir: ./.conduit/sources
"#;
        let cfg: ConduitConfig = serde_yaml::from_str(yaml).unwrap();
        cfg.validate().unwrap();
        assert_eq!(cfg.sources.len(), 2);
        assert_eq!(cfg.sources[0].id(), "inbox");
        assert_eq!(cfg.sources[1].id(), "pipe");
    }

    #[test]
    fn rejects_duplicate_source_id() {
        let yaml = r#"
version: 1
routing: { file: routing.json }
adapters:
  - type: sqlite
    id: s
    priority: 1
    config: { path: ":memory:" }
sources:
  - { type: stdin, id: dup, state_dir: /tmp }
  - { type: stdin, id: dup, state_dir: /tmp }
"#;
        let cfg: ConduitConfig = serde_yaml::from_str(yaml).unwrap();
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("duplicate source id"), "{err}");
    }

    #[test]
    fn config_without_sources_is_valid() {
        let yaml = r#"
version: 1
routing: { file: routing.json }
adapters:
  - type: sqlite
    id: s
    priority: 1
    config: { path: ":memory:" }
"#;
        let cfg: ConduitConfig = serde_yaml::from_str(yaml).unwrap();
        cfg.validate().unwrap();
        assert!(cfg.sources.is_empty());
    }
}
