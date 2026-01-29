use core::fmt;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct ConduitConfig {
    pub version: u32,
    pub routing: RoutingConfig,
    pub adapters: Vec<AdapterConfig>,
}

impl ConduitConfig {
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.version != 1 {
            return Err(ConfigError::UnsupportedVersion(self.version));
        }

        if self.adapters.is_empty() {
            return Err(ConfigError::NoAdaptersDefined);
        }

        let mut ids = std::collections::HashSet::new();
        let mut has_sqlite = false;
        let mut has_file = false;

        for adapter in &self.adapters {
            let id = adapter.id();
            if !ids.insert(id.to_string()) {
                return Err(ConfigError::DuplicateAdapterId(id.to_string()));
            }

            match adapter {
                AdapterConfig::Sqlite(_) => {
                    if has_sqlite {
                        return Err(ConfigError::MultipleAdaptersOfSameType("sqlite"));
                    }
                    has_sqlite = true;
                }
                AdapterConfig::File(_) => {
                    if has_file {
                        return Err(ConfigError::MultipleAdaptersOfSameType("file"));
                    }
                    has_file = true;
                }
            }
        }

        Ok(())
    }
}

#[derive(Debug, Deserialize)]
pub struct RoutingConfig {
    pub file: String,
}

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
}

#[derive(Debug, Deserialize)]
pub struct FileConfig {
    pub root: String,
}

#[derive(Debug)]
pub enum ConfigError {
    UnsupportedVersion(u32),
    NoAdaptersDefined,
    DuplicateAdapterId(String),
    MultipleAdaptersOfSameType(&'static str),
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
            ConfigError::MultipleAdaptersOfSameType(str) => {
                write!(f, "multiple adapter of type: {}", str)
            }
        }
    }
}

impl std::error::Error for ConfigError {}

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
}
