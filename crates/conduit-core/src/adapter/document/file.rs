use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::{
    adapter::document::adapter::DocumentError,
    adapter::document::runtime::DocumentRuntimeBuilder,
    adapter::{
        AdapterError, AdapterResult, GuardState, SkipReason, StorageAdapter, WriteDecision, decide,
    },
    event::Event,
    routing::StorageKind,
    runtime::config::MigrationPolicy,
    upcast::UpcasterRegistry,
};

/// Idempotency guard sidecar (Phase 11.3 marker; Phase 12.4 sequence-gated
/// upsert; Phase 13.3 tombstone). Never removed once written — a `deleted`
/// sidecar is a tombstone that still gates later events for this entity.
#[derive(Debug, Serialize, Deserialize)]
struct ProjectionGuard {
    last_sequence: u64,
    last_event_id: String,
    #[serde(default)]
    deleted: bool,
    #[serde(default)]
    permanent: bool,
}

impl From<&ProjectionGuard> for GuardState {
    fn from(g: &ProjectionGuard) -> Self {
        GuardState {
            last_sequence: g.last_sequence,
            deleted: g.deleted,
            permanent: g.permanent,
        }
    }
}

/// File-based document adapter (Phase 4: idempotent; Phase 12: sequence-gated
/// upsert; Phase 13: sequence-gated delete & tombstones)
pub struct FileDocumentAdapter {
    id: String,
    priority: u32,
    root: PathBuf,
    builder: DocumentRuntimeBuilder,
    upcasters: Arc<UpcasterRegistry>,
    migration_policy: MigrationPolicy,
}

impl FileDocumentAdapter {
    pub fn new(
        id: String,
        root: PathBuf,
        priority: u32,
        builder: DocumentRuntimeBuilder,
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
        AdapterResult::failure(self.id.clone(), StorageKind::Document, error)
    }

    /// Entity output path, keyed by `(collection, entity_id)` (Phase 13.3) —
    /// the mapping's stable target identity, not `event.event_type`. A delete
    /// mapping has its own event type (`OrderCancelled` vs `OrderCreated`)
    /// but the same `collection`, so it points at the same file its create
    /// mapping wrote.
    fn output_path(&self, collection: &str, entity_id: &str) -> PathBuf {
        self.root
            .join(collection)
            .join(format!("{}.json", entity_id))
    }

    /// Idempotency guard path, keyed by `(collection, entity_id)` (Phase
    /// 11.3, re-keyed by collection in Phase 13.3) — not `event_id` or
    /// `event_type`, so every event type touching one entity shares one guard.
    fn guard_path(&self, collection: &str, entity_id: &str) -> PathBuf {
        self.root
            .join(".conduit")
            .join("entities")
            .join(collection)
            .join(format!("{}.done", entity_id))
    }

    /// Read the guard sidecar if present. `Ok(None)` means no entity has been
    /// projected here yet; a corrupt sidecar is a write failure, not treated
    /// as absent (silently forgetting `last_sequence`/`deleted` would let a
    /// stale event through, or un-delete a tombstoned entity).
    fn read_guard(&self, guard: &Path) -> std::io::Result<Option<ProjectionGuard>> {
        match std::fs::read_to_string(guard) {
            Ok(content) => {
                let parsed: ProjectionGuard = serde_json::from_str(&content)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
                Ok(Some(parsed))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Commit the guard via write-to-temp-then-rename, so a reader can never
    /// observe a partially-written guard file (closes the check-then-write
    /// race the old direct `fs::write` guard had).
    fn commit_guard(&self, guard: &Path, state: &ProjectionGuard) -> std::io::Result<()> {
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

impl StorageAdapter for FileDocumentAdapter {
    fn kind(&self) -> StorageKind {
        StorageKind::Document
    }

    fn id(&self) -> &str {
        &self.id
    }

    fn priority(&self) -> u32 {
        self.priority
    }

    fn handle(&self, event: &Event) -> AdapterResult {
        // Build document projection (upcasting the payload to the mapping's target
        // version first). Entity identity is resolved as part of the projection
        // (Phase 11.1), so the idempotency guard below is checked by entity —
        // it can't be computed any earlier than this.
        let projection = match self.builder.build(event, &self.upcasters) {
            Ok(p) => p,
            Err(DocumentError::UnsupportedVersion {
                from_version,
                to_version,
                reason,
                ..
            }) => {
                return match self.migration_policy {
                    MigrationPolicy::Strict => AdapterResult::unsupported_version(
                        self.id.clone(),
                        StorageKind::Document,
                        reason,
                        from_version,
                        to_version,
                    ),
                    MigrationPolicy::IgnoreUnmatched => AdapterResult::skipped_versioned(
                        self.id.clone(),
                        StorageKind::Document,
                        SkipReason::UnsupportedVersion,
                        from_version,
                        to_version,
                    ),
                };
            }
            Err(e) => return self.failure(AdapterError::WriteFailed(e.to_string())),
        };

        let guard_path = self.guard_path(&projection.collection, &projection.entity_id);

        // PHASE 11.3/12.4/13.3: read the guard sidecar for this entity — read
        // -decide-write. Known limitation (Phase 12 non-goal): this is not
        // atomic across processes; the file adapter assumes a single writer.
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
                    StorageKind::Document,
                    SkipReason::AlreadyProjected,
                    projection.source_version,
                    projection.projected_version,
                );
            }
            WriteDecision::SkipStale => {
                return AdapterResult::skipped_versioned(
                    self.id.clone(),
                    StorageKind::Document,
                    SkipReason::StaleSequence,
                    projection.source_version,
                    projection.projected_version,
                );
            }
            WriteDecision::SkipTombstoned => {
                return AdapterResult::skipped_versioned(
                    self.id.clone(),
                    StorageKind::Document,
                    SkipReason::Tombstoned,
                    projection.source_version,
                    projection.projected_version,
                );
            }
            WriteDecision::SkipAlreadyDeleted => {
                // Phase 13.3: still bump last_sequence so a later, truly
                // out-of-order resurrection attempt compares against the
                // highest delete sequence seen, not a stale one.
                let bumped = ProjectionGuard {
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
                    StorageKind::Document,
                    SkipReason::AlreadyDeleted,
                    projection.source_version,
                    projection.projected_version,
                );
            }
            WriteDecision::Insert | WriteDecision::Update | WriteDecision::Delete => {}
        }

        let out_path = self.output_path(&projection.collection, &projection.entity_id);

        let write_result: Result<(), Box<dyn std::error::Error>> = (|| {
            if decision == WriteDecision::Delete {
                match std::fs::remove_file(&out_path) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e.into()),
                }
            } else {
                if let Some(parent) = out_path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(
                    &out_path,
                    serde_json::to_string_pretty(&projection.document)?,
                )?;
            }
            Ok(())
        })();

        if let Err(e) = write_result {
            return self.failure(AdapterError::WriteFailed(e.to_string()));
        }

        // PHASE 11.3/12.4/13.3: commit the guard AFTER the write, via atomic
        // rename. `deleted` is set whenever this write was a `Delete` —
        // the tombstone is never removed, just like the SQL guard row.
        let new_guard = ProjectionGuard {
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
                StorageKind::Document,
                projection.source_version,
                projection.projected_version,
            ),
            WriteDecision::Update => AdapterResult::updated_versioned(
                self.id.clone(),
                StorageKind::Document,
                projection.source_version,
                projection.projected_version,
            ),
            WriteDecision::Delete => AdapterResult::deleted_versioned(
                self.id.clone(),
                StorageKind::Document,
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
