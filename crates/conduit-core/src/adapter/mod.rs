pub mod document;
pub mod sql;

use core::fmt;

use crate::event::Event;
use crate::routing::StorageKind;

#[derive(Debug)]
pub enum AdapterError {
    WriteFailed(String),
    Skipped(String),
}

impl fmt::Display for AdapterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AdapterError::WriteFailed(msg) => write!(f, "{}", msg),
            AdapterError::Skipped(msg) => write!(f, "{}", msg),
        }
    }
}

impl std::error::Error for AdapterError {}

#[derive(Debug)]
pub struct AdapterResult {
    pub adapter_id: String,
    pub kind: StorageKind,
    pub success: bool,
    pub error: Option<AdapterError>,
}

pub trait StorageAdapter {
    /// Logical storage family (Sql / Document)
    fn kind(&self) -> StorageKind;

    /// Stable adapter identifier (from config)
    fn id(&self) -> &str;

    /// Execution priority (lower = earlier)
    fn priority(&self) -> u32;

    /// Execute side effect
    fn handle(&self, event: &Event) -> AdapterResult;
}
