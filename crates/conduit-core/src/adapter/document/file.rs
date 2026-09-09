use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::{
    adapter::document::adapter::DocumentError,
    adapter::document::runtime::DocumentRuntimeBuilder,
    adapter::{AdapterError, AdapterResult, SkipReason, StorageAdapter, WriteDecision, decide},
    event::Event,
    routing::StorageKind,
    runtime::config::MigrationPolicy,
    upcast::UpcasterRegistry,
};

/// Idempotency guard sidecar (Phase 11.3 marker, Phase 12.4 sequence-gated
/// upsert): `last_sequence` is what `decide()` compares `event.sequence`
/// against for `on_existing: replace` mappings.
#[derive(Debug, Serialize, Deserialize)]
struct ProjectionGuard {
    last_sequence: u64,
    last_event_id: String,
}

/// File-based document adapter (Phase 4: idempotent; Phase 12: sequence-gated upsert)
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

    /// Idempotency guard path, keyed by `(event_type, entity_id)` (Phase 11.3) —
    /// not `event_id`, so a redelivered creation event with a new `event_id`
    /// for the same entity is still recognized as already processed.
    fn guard_path(&self, event_type: &str, entity_id: &str) -> PathBuf {
        self.root
            .join(".conduit")
            .join("entities")
            .join(event_type)
            .join(format!("{}.done", entity_id))
    }

    /// Read the guard sidecar if present. `Ok(None)` means no entity has been
    /// projected here yet; a corrupt sidecar is a write failure, not treated
    /// as absent (silently forgetting `last_sequence` would let a stale event
    /// through under `on_existing: replace`).
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

        let guard_path = self.guard_path(&event.event_type, &projection.entity_id);

        // PHASE 11.3/12.4: read the guard sidecar for this entity — read
        // -decide-write. Known limitation (Phase 12 non-goal): this is not
        // atomic across processes; the file adapter assumes a single writer.
        let stored_guard = match self.read_guard(&guard_path) {
            Ok(g) => g,
            Err(e) => return self.failure(AdapterError::WriteFailed(e.to_string())),
        };
        let exists = stored_guard.is_some();
        let stored_last_sequence = stored_guard.map(|g| g.last_sequence);

        let is_update = match decide(
            projection.on_existing,
            exists,
            stored_last_sequence,
            event.sequence,
        ) {
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
            WriteDecision::Insert => false,
            WriteDecision::Update => true,
        };

        // --------------------------------------------------
        // Output path keyed by entity, not event (Phase 11.1/11.3). For
        // `on_existing: replace`, `Update` overwrites this same path with the
        // full new projected state (Phase 12.4) — no partial-column writes.
        // --------------------------------------------------
        let write_result: Result<(), Box<dyn std::error::Error>> = (|| {
            let out_path = self
                .root
                .join(&event.event_type)
                .join(format!("{}.json", projection.entity_id));

            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent)?;
            }

            std::fs::write(
                &out_path,
                serde_json::to_string_pretty(&projection.document)?,
            )?;

            Ok(())
        })();

        if let Err(e) = write_result {
            return self.failure(AdapterError::WriteFailed(e.to_string()));
        }

        // PHASE 11.3/12.4: commit the guard AFTER the write, via atomic rename.
        let new_guard = ProjectionGuard {
            last_sequence: event.sequence,
            last_event_id: event.event_id.clone(),
        };
        if let Err(e) = self.commit_guard(&guard_path, &new_guard) {
            return self.failure(AdapterError::WriteFailed(e.to_string()));
        }

        if is_update {
            AdapterResult::updated_versioned(
                self.id.clone(),
                StorageKind::Document,
                projection.source_version,
                projection.projected_version,
            )
        } else {
            AdapterResult::created_versioned(
                self.id.clone(),
                StorageKind::Document,
                projection.source_version,
                projection.projected_version,
            )
        }
    }
}
