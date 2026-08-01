pub mod document;
pub mod sql;

use core::fmt;

use crate::event::Event;
use crate::routing::StorageKind;

#[derive(Debug)]
pub enum AdapterError {
    WriteFailed(String),
    Skipped(String),
    /// No upcaster chain to the mapping's target version, under `MigrationPolicy::Strict`.
    UnsupportedVersion(String),
}

impl fmt::Display for AdapterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AdapterError::WriteFailed(msg) => write!(f, "{}", msg),
            AdapterError::Skipped(msg) => write!(f, "{}", msg),
            AdapterError::UnsupportedVersion(msg) => write!(f, "{}", msg),
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
    /// Event payload version this adapter observed (Phase 10.3).
    pub source_version: Option<u32>,
    /// Mapping's target schema version this adapter projected into (Phase 10.3).
    pub projected_version: Option<u32>,
}

impl AdapterResult {
    pub fn success(adapter_id: String, kind: StorageKind) -> Self {
        Self {
            adapter_id,
            kind,
            success: true,
            error: None,
            source_version: None,
            projected_version: None,
        }
    }

    pub fn success_versioned(
        adapter_id: String,
        kind: StorageKind,
        source_version: u32,
        projected_version: u32,
    ) -> Self {
        Self {
            adapter_id,
            kind,
            success: true,
            error: None,
            source_version: Some(source_version),
            projected_version: Some(projected_version),
        }
    }

    pub fn failure(adapter_id: String, kind: StorageKind, error: AdapterError) -> Self {
        Self {
            adapter_id,
            kind,
            success: false,
            error: Some(error),
            source_version: None,
            projected_version: None,
        }
    }

    pub fn skipped(adapter_id: String, kind: StorageKind, message: String) -> Self {
        Self {
            adapter_id,
            kind,
            success: true,
            error: Some(AdapterError::Skipped(message)),
            source_version: None,
            projected_version: None,
        }
    }

    /// `MigrationPolicy::IgnoreUnmatched`: no upcaster chain, but the batch continues.
    pub fn skipped_version(
        adapter_id: String,
        kind: StorageKind,
        message: String,
        source_version: u32,
        projected_version: u32,
    ) -> Self {
        Self {
            adapter_id,
            kind,
            success: true,
            error: Some(AdapterError::Skipped(message)),
            source_version: Some(source_version),
            projected_version: Some(projected_version),
        }
    }

    /// `MigrationPolicy::Strict`: no upcaster chain; the adapter (and, under
    /// fail-fast, the batch) fails immediately.
    pub fn unsupported_version(
        adapter_id: String,
        kind: StorageKind,
        message: String,
        source_version: u32,
        projected_version: u32,
    ) -> Self {
        Self {
            adapter_id,
            kind,
            success: false,
            error: Some(AdapterError::UnsupportedVersion(message)),
            source_version: Some(source_version),
            projected_version: Some(projected_version),
        }
    }
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
