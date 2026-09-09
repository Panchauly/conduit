pub mod document;
pub mod sql;

use core::fmt;

use serde::{Deserialize, Serialize};

use crate::event::Event;
use crate::routing::StorageKind;

/// Adapter-level execution failure. Deliberately narrow — everything that
/// isn't a hard failure (idempotent skip, stale-sequence skip, version
/// mismatch under `IgnoreUnmatched`) is a [`SkipReason`] on [`AdapterOutcome`]
/// instead, not an error (Phase 12.1).
#[derive(Debug)]
pub enum AdapterError {
    WriteFailed(String),
    /// No upcaster chain to the mapping's target version, under `MigrationPolicy::Strict`.
    UnsupportedVersion(String),
}

impl fmt::Display for AdapterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AdapterError::WriteFailed(msg) => write!(f, "{}", msg),
            AdapterError::UnsupportedVersion(msg) => write!(f, "{}", msg),
        }
    }
}

impl std::error::Error for AdapterError {}

/// Why an adapter skipped an event without writing anything (Phase 12.1).
/// Machine-readable — call sites match on this instead of string-sniffing a
/// message, and it round-trips through [`crate::execution::AdapterOutcome`]
/// into the JSON execution report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkipReason {
    /// Phase 11 idempotency: this entity already has a guard row/marker, and
    /// the mapping's `on_existing` is `ignore` (the default).
    AlreadyProjected,
    /// Phase 12.2: `on_existing: replace`, but `event.sequence` is not newer
    /// than the guard's `last_sequence` — a redelivery or a superseded update.
    StaleSequence,
    /// `MigrationPolicy::IgnoreUnmatched`: no upcaster chain to the mapping's
    /// target version; the event is skipped rather than failing the batch.
    UnsupportedVersion,
}

/// Outcome of a single adapter invocation (Phase 12.1). Replaces the old
/// `success: bool` + `AdapterError::Skipped(String)` encoding — six distinct
/// results (created, updated, three skip reasons, failed) as one enum instead
/// of a boolean plus string-sniffing.
#[derive(Debug)]
pub enum AdapterOutcome {
    /// A new entity was written (`on_existing` is irrelevant — nothing existed yet).
    Created,
    /// An existing entity was overwritten (`on_existing: replace` only).
    Updated,
    /// Nothing was written; see [`SkipReason`] for why.
    Skipped(SkipReason),
    /// The adapter failed; see [`AdapterError`] for why.
    Failed(AdapterError),
}

#[derive(Debug)]
pub struct AdapterResult {
    pub adapter_id: String,
    pub kind: StorageKind,
    pub outcome: AdapterOutcome,
    /// Event payload version this adapter observed (Phase 10.3).
    pub source_version: Option<u32>,
    /// Mapping's target schema version this adapter projected into (Phase 10.3).
    pub projected_version: Option<u32>,
}

impl AdapterResult {
    /// True for every outcome except [`AdapterOutcome::Failed`] (created, updated, and skipped are all non-failures).
    pub fn is_success(&self) -> bool {
        !matches!(self.outcome, AdapterOutcome::Failed(_))
    }

    pub fn is_skipped(&self) -> bool {
        matches!(self.outcome, AdapterOutcome::Skipped(_))
    }

    pub fn created(adapter_id: String, kind: StorageKind) -> Self {
        Self {
            adapter_id,
            kind,
            outcome: AdapterOutcome::Created,
            source_version: None,
            projected_version: None,
        }
    }

    pub fn created_versioned(
        adapter_id: String,
        kind: StorageKind,
        source_version: u32,
        projected_version: u32,
    ) -> Self {
        Self {
            adapter_id,
            kind,
            outcome: AdapterOutcome::Created,
            source_version: Some(source_version),
            projected_version: Some(projected_version),
        }
    }

    /// Phase 12.3/12.4: an existing entity's projected state was overwritten.
    pub fn updated_versioned(
        adapter_id: String,
        kind: StorageKind,
        source_version: u32,
        projected_version: u32,
    ) -> Self {
        Self {
            adapter_id,
            kind,
            outcome: AdapterOutcome::Updated,
            source_version: Some(source_version),
            projected_version: Some(projected_version),
        }
    }

    pub fn failure(adapter_id: String, kind: StorageKind, error: AdapterError) -> Self {
        Self {
            adapter_id,
            kind,
            outcome: AdapterOutcome::Failed(error),
            source_version: None,
            projected_version: None,
        }
    }

    pub fn skipped(adapter_id: String, kind: StorageKind, reason: SkipReason) -> Self {
        Self {
            adapter_id,
            kind,
            outcome: AdapterOutcome::Skipped(reason),
            source_version: None,
            projected_version: None,
        }
    }

    /// Skip carrying the observed/target schema versions (idempotent replay,
    /// stale-sequence rejection, or `MigrationPolicy::IgnoreUnmatched`).
    pub fn skipped_versioned(
        adapter_id: String,
        kind: StorageKind,
        reason: SkipReason,
        source_version: u32,
        projected_version: u32,
    ) -> Self {
        Self {
            adapter_id,
            kind,
            outcome: AdapterOutcome::Skipped(reason),
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
            outcome: AdapterOutcome::Failed(AdapterError::UnsupportedVersion(message)),
            source_version: Some(source_version),
            projected_version: Some(projected_version),
        }
    }
}

// ---------------------------------------------------------------------------
// Phase 12.2: write mode & the gated decision
// ---------------------------------------------------------------------------

/// Mapping-level write mode for an entity that already has a projected state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnExisting {
    /// Entity already projected → clean skip (`SkipReason::AlreadyProjected`).
    /// Identical to Phase 11 behavior; the default, so every mapping written
    /// before Phase 12 is unchanged.
    #[default]
    Ignore,
    /// Entity already projected → overwrite the whole projected state, gated
    /// by `sequence` (Phase 12).
    Replace,
}

/// What an adapter should do for one event, given the entity's current guard
/// state. Pure and side-effect free — no I/O, no adapter, exhaustively unit
/// tested below.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteDecision {
    Insert,
    Update,
    SkipIdempotent,
    SkipStale,
}

/// The Phase 12.2 gated decision.
///
/// | mode      | exists? | `event_seq` vs `stored_last_seq` | decision       |
/// |-----------|---------|-----------------------------------|----------------|
/// | `ignore`  | no      | —                                   | `Insert`         |
/// | `ignore`  | yes     | —                                   | `SkipIdempotent` |
/// | `replace` | no      | —                                   | `Insert`         |
/// | `replace` | yes     | `event_seq > stored`                | `Update`         |
/// | `replace` | yes     | `event_seq <= stored`                | `SkipStale`      |
pub fn decide(
    mode: OnExisting,
    exists: bool,
    stored_last_seq: Option<u64>,
    event_seq: u64,
) -> WriteDecision {
    match (mode, exists) {
        (OnExisting::Ignore, false) => WriteDecision::Insert,
        (OnExisting::Ignore, true) => WriteDecision::SkipIdempotent,
        (OnExisting::Replace, false) => WriteDecision::Insert,
        (OnExisting::Replace, true) => {
            // `exists` implies a guard row was found, so `stored_last_seq` is
            // always `Some` in practice; treat a missing value as 0 (any
            // sequence wins) rather than panic on a caller's bookkeeping bug.
            if event_seq > stored_last_seq.unwrap_or(0) {
                WriteDecision::Update
            } else {
                WriteDecision::SkipStale
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Shared value helpers (Phase 11.1)
// ---------------------------------------------------------------------------

/// Canonical string form of a resolved JSON leaf value (Phase 11.1). Shared by
/// the SQL adapter (legacy string-typed bind params) and the document adapter
/// (`entity_id` guard key / output filename) so the same payload/metadata
/// value always canonicalizes the same way across adapters. Objects and
/// arrays fall back to their JSON text — mapping authors should not use those
/// as an identity or column value (identity paths reject them outright; see
/// [`is_identity_scalar`]).
pub(crate) fn json_scalar_to_string(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// True for the value types Phase 11 accepts as an entity-identity component
/// (a document `id`, or one column of a SQL `primary_key`): string, number,
/// bool. Objects, arrays, and null are rejected — a path that resolves to the
/// wrong place should fail the build fast, not silently produce `""` or a
/// JSON blob inside a guard key.
pub(crate) fn is_identity_scalar(value: &serde_json::Value) -> bool {
    matches!(
        value,
        serde_json::Value::String(_) | serde_json::Value::Number(_) | serde_json::Value::Bool(_)
    )
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignore_absent_inserts() {
        assert_eq!(
            decide(OnExisting::Ignore, false, None, 1),
            WriteDecision::Insert
        );
    }

    #[test]
    fn ignore_present_skips_idempotent_regardless_of_sequence() {
        assert_eq!(
            decide(OnExisting::Ignore, true, Some(99), 1),
            WriteDecision::SkipIdempotent
        );
    }

    #[test]
    fn replace_absent_inserts() {
        assert_eq!(
            decide(OnExisting::Replace, false, None, 1),
            WriteDecision::Insert
        );
    }

    #[test]
    fn replace_present_with_newer_sequence_updates() {
        assert_eq!(
            decide(OnExisting::Replace, true, Some(5), 6),
            WriteDecision::Update
        );
    }

    #[test]
    fn replace_present_with_equal_sequence_is_stale() {
        assert_eq!(
            decide(OnExisting::Replace, true, Some(5), 5),
            WriteDecision::SkipStale
        );
    }

    #[test]
    fn replace_present_with_older_sequence_is_stale() {
        assert_eq!(
            decide(OnExisting::Replace, true, Some(5), 4),
            WriteDecision::SkipStale
        );
    }
}
