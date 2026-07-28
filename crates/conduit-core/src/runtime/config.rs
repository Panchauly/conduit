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

// ------------------------------------------------------------
// Errors
// ------------------------------------------------------------

#[derive(Debug)]
pub enum ConfigError {
    UnsupportedVersion(u32),
    NoAdaptersDefined,
    DuplicateAdapterId(String),
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
            ConfigError::SelfDependency(id) => {
                write!(f, "adapter {:?} cannot depend on itself", id)
            }
            ConfigError::UnknownDependency { adapter, dependency } => {
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
        }
    }

    pub fn priority(&self) -> u32 {
        match self {
            AdapterConfig::Sqlite(cfg) => cfg.priority,
            AdapterConfig::File(cfg) => cfg.priority,
        }
    }

    /// Declared capabilities for this adapter (if any).
    pub fn capabilities(&self) -> Option<&[AdapterCapability]> {
        match self {
            AdapterConfig::Sqlite(cfg) => cfg.capabilities.as_deref(),
            AdapterConfig::File(cfg) => cfg.capabilities.as_deref(),
        }
    }

    pub fn depends_on(&self) -> &[AdapterId] {
        match self {
            AdapterConfig::Sqlite(cfg) => &cfg.depends_on,
            AdapterConfig::File(cfg) => &cfg.depends_on,
        }
    }
}
