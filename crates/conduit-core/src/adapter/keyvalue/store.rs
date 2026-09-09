use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::{
    adapter::keyvalue::adapter::KvError,
    adapter::keyvalue::runtime::KvRuntimeBuilder,
    adapter::{
        AdapterError, AdapterResult, GuardState, SkipReason, StorageAdapter, WriteDecision, decide,
    },
    event::Event,
    routing::StorageKind,
    runtime::config::MigrationPolicy,
    upcast::UpcasterRegistry,
};

/// Idempotency guard sidecar — structurally identical to the document
/// adapter's `ProjectionGuard` (Phase 11.3/12.4/13.3), just named for the
/// third storage kind.
#[derive(Debug, Serialize, Deserialize)]
struct KvGuard {
    last_sequence: u64,
    last_event_id: String,
    #[serde(default)]
    deleted: bool,
    #[serde(default)]
    permanent: bool,
}

impl From<&KvGuard> for GuardState {
    fn from(g: &KvGuard) -> Self {
        GuardState {
            last_sequence: g.last_sequence,
            deleted: g.deleted,
            permanent: g.permanent,
        }
    }
}

/// File-backed key-value store (Phase 14) — the third `StorageAdapter`,
/// exercising the exact same `decide()` gate, `AdapterOutcome`, and
/// `AdapterCapability` machinery as SQL and document with zero core changes.
pub struct KeyValueStore {
    id: String,
    priority: u32,
    root: PathBuf,
    builder: KvRuntimeBuilder,
    upcasters: Arc<UpcasterRegistry>,
    migration_policy: MigrationPolicy,
}

impl KeyValueStore {
    pub fn new(
        id: String,
        root: PathBuf,
        priority: u32,
        builder: KvRuntimeBuilder,
        upcasters: Arc<UpcasterRegistry>,
        migration_policy: MigrationPolicy,
    ) -> Self {
        Self {
            id,
            root,
            priority,
            builder,
            upcasters,
            migration_policy,
        }
    }

    fn failure(&self, error: AdapterError) -> AdapterResult {
        AdapterResult::failure(self.id.clone(), StorageKind::KeyValue, error)
    }

    /// Value path, keyed by `(namespace, key)` (Phase 14.2).
    fn value_path(&self, namespace: &str, key: &str) -> PathBuf {
        self.root.join(namespace).join(format!("{}.json", key))
    }

    /// Idempotency guard path, keyed by `(namespace, key)`.
    fn guard_path(&self, namespace: &str, key: &str) -> PathBuf {
        self.root
            .join(".conduit")
            .join(namespace)
            .join(format!("{}.guard.json", key))
    }

    /// Read the guard sidecar if present. `Ok(None)` means this key has never
    /// been projected here; a corrupt sidecar is a write failure, not treated
    /// as absent (silently forgetting `last_sequence`/`deleted` would let a
    /// stale event through, or un-delete a tombstoned key).
    fn read_guard(&self, guard: &Path) -> std::io::Result<Option<KvGuard>> {
        match std::fs::read_to_string(guard) {
            Ok(content) => {
                let parsed: KvGuard = serde_json::from_str(&content)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
                Ok(Some(parsed))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Commit the guard via write-to-temp-then-rename (Phase 11.3 pattern),
    /// so a reader can never observe a partially-written guard file.
    fn commit_guard(&self, guard: &Path, state: &KvGuard) -> std::io::Result<()> {
        let parent = guard.parent().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "idempotency guard path has no parent directory",
            )
        })?;
        std::fs::create_dir_all(parent)?;

        let tmp_name = format!(
            ".{}.tmp-{}",
            guard
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("guard"),
            std::process::id()
        );
        let tmp = parent.join(tmp_name);
        let content = serde_json::to_string(state)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(&tmp, content)?;
        std::fs::rename(&tmp, guard)
    }
}

impl StorageAdapter for KeyValueStore {
    fn kind(&self) -> StorageKind {
        StorageKind::KeyValue
    }

    fn id(&self) -> &str {
        &self.id
    }

    fn priority(&self) -> u32 {
        self.priority
    }

    fn handle(&self, event: &Event) -> AdapterResult {
        // Build the projection (upcasting the payload to the mapping's target
        // version first). Entity identity is resolved as part of the
        // projection, so the idempotency guard below is checked by key — it
        // can't be computed any earlier than this.
        let projection = match self.builder.build(event, &self.upcasters) {
            Ok(p) => p,
            Err(KvError::UnsupportedVersion {
                from_version,
                to_version,
                reason,
                ..
            }) => {
                return match self.migration_policy {
                    MigrationPolicy::Strict => AdapterResult::unsupported_version(
                        self.id.clone(),
                        StorageKind::KeyValue,
                        reason,
                        from_version,
                        to_version,
                    ),
                    MigrationPolicy::IgnoreUnmatched => AdapterResult::skipped_versioned(
                        self.id.clone(),
                        StorageKind::KeyValue,
                        SkipReason::UnsupportedVersion,
                        from_version,
                        to_version,
                    ),
                };
            }
            Err(e) => return self.failure(AdapterError::WriteFailed(e.to_string())),
        };

        let guard_path = self.guard_path(&projection.namespace, &projection.entity_key);

        // Read-decide-write, all against this one sidecar file. Known
        // limitation (Phase 12.4 non-goal, inherited): not atomic across
        // processes — the store assumes a single writer.
        let stored_guard = match self.read_guard(&guard_path) {
            Ok(g) => g,
            Err(e) => return self.failure(AdapterError::WriteFailed(e.to_string())),
        };
        let stored: Option<GuardState> = stored_guard.as_ref().map(GuardState::from);

        let decision = decide(
            projection.operation,
            projection.on_existing,
            stored,
            event.sequence,
        );

        match decision {
            WriteDecision::SkipIdempotent => {
                return AdapterResult::skipped_versioned(
                    self.id.clone(),
                    StorageKind::KeyValue,
                    SkipReason::AlreadyProjected,
                    projection.source_version,
                    projection.projected_version,
                );
            }
            WriteDecision::SkipStale => {
                return AdapterResult::skipped_versioned(
                    self.id.clone(),
                    StorageKind::KeyValue,
                    SkipReason::StaleSequence,
                    projection.source_version,
                    projection.projected_version,
                );
            }
            WriteDecision::SkipTombstoned => {
                return AdapterResult::skipped_versioned(
                    self.id.clone(),
                    StorageKind::KeyValue,
                    SkipReason::Tombstoned,
                    projection.source_version,
                    projection.projected_version,
                );
            }
            WriteDecision::SkipAlreadyDeleted => {
                // Still bump last_sequence so a later, truly out-of-order
                // resurrection attempt compares against the highest delete
                // sequence seen, not a stale one.
                let bumped = KvGuard {
                    last_sequence: event.sequence,
                    last_event_id: event.event_id.clone(),
                    deleted: true,
                    permanent: false,
                };
                if let Err(e) = self.commit_guard(&guard_path, &bumped) {
                    return self.failure(AdapterError::WriteFailed(e.to_string()));
                }
                return AdapterResult::skipped_versioned(
                    self.id.clone(),
                    StorageKind::KeyValue,
                    SkipReason::AlreadyDeleted,
                    projection.source_version,
                    projection.projected_version,
                );
            }
            WriteDecision::Insert | WriteDecision::Update | WriteDecision::Delete => {}
        }

        let value_path = self.value_path(&projection.namespace, &projection.entity_key);

        let write_result: Result<(), Box<dyn std::error::Error>> = (|| {
            if decision == WriteDecision::Delete {
                match std::fs::remove_file(&value_path) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e.into()),
                }
            } else {
                if let Some(parent) = value_path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(
                    &value_path,
                    serde_json::to_string_pretty(&projection.value)?,
                )?;
            }
            Ok(())
        })();

        if let Err(e) = write_result {
            return self.failure(AdapterError::WriteFailed(e.to_string()));
        }

        // Commit the guard AFTER the write, via atomic rename. `deleted` is
        // set whenever this write was a `Delete` — the tombstone is never
        // removed.
        let new_guard = KvGuard {
            last_sequence: event.sequence,
            last_event_id: event.event_id.clone(),
            deleted: decision == WriteDecision::Delete,
            permanent: decision == WriteDecision::Delete && projection.permanent,
        };
        if let Err(e) = self.commit_guard(&guard_path, &new_guard) {
            return self.failure(AdapterError::WriteFailed(e.to_string()));
        }

        match decision {
            WriteDecision::Insert => AdapterResult::created_versioned(
                self.id.clone(),
                StorageKind::KeyValue,
                projection.source_version,
                projection.projected_version,
            ),
            WriteDecision::Update => AdapterResult::updated_versioned(
                self.id.clone(),
                StorageKind::KeyValue,
                projection.source_version,
                projection.projected_version,
            ),
            WriteDecision::Delete => AdapterResult::deleted_versioned(
                self.id.clone(),
                StorageKind::KeyValue,
                projection.source_version,
                projection.projected_version,
            ),
            // Every skip variant returned early above.
            WriteDecision::SkipIdempotent
            | WriteDecision::SkipStale
            | WriteDecision::SkipAlreadyDeleted
            | WriteDecision::SkipTombstoned => self.failure(AdapterError::WriteFailed(
                "unreachable: skip decision reached the write path".to_string(),
            )),
        }
    }
}
