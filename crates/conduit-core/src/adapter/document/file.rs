use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::{
    adapter::document::adapter::DocumentError,
    adapter::document::runtime::DocumentRuntimeBuilder,
    adapter::{AdapterError, AdapterResult, StorageAdapter},
    event::Event,
    routing::StorageKind,
    runtime::config::MigrationPolicy,
    upcast::UpcasterRegistry,
};

/// File-based document adapter (Phase 4: idempotent)
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

    /// Commit the guard via write-to-temp-then-rename, so a reader can never
    /// observe a partially-written guard file (closes the check-then-write
    /// race the old direct `fs::write` guard had).
    fn commit_guard(&self, guard: &Path) -> std::io::Result<()> {
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
        std::fs::write(&tmp, b"ok")?;
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
                    MigrationPolicy::IgnoreUnmatched => AdapterResult::skipped_version(
                        self.id.clone(),
                        StorageKind::Document,
                        reason,
                        from_version,
                        to_version,
                    ),
                };
            }
            Err(e) => return self.failure(AdapterError::WriteFailed(e.to_string())),
        };

        let guard = self.guard_path(&event.event_type, &projection.entity_id);

        // --------------------------------------------------
        // PHASE 11.3: entity-aware idempotency check (BEFORE write). Supersedes
        // the Phase 4 event_id-keyed guard: a redelivered creation event with a
        // new event_id for the same entity is skipped here too, not just an
        // exact event_id replay.
        // --------------------------------------------------
        if guard.exists() {
            return AdapterResult::skipped(
                self.id.clone(),
                StorageKind::Document,
                format!("entity '{}' already projected", projection.entity_id),
            );
        }

        // --------------------------------------------------
        // Output path keyed by entity, not event (Phase 11.1/11.3): a
        // redelivered creation event with a new event_id for the same entity
        // overwrites nothing new — the guard above already caught it — and a
        // legitimate first write always lands at the entity's own path.
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

        // --------------------------------------------------
        // PHASE 11.3: commit the guard AFTER the write, via atomic rename.
        // --------------------------------------------------
        if let Err(e) = self.commit_guard(&guard) {
            return self.failure(AdapterError::WriteFailed(e.to_string()));
        }

        AdapterResult::success_versioned(
            self.id.clone(),
            StorageKind::Document,
            projection.source_version,
            projection.projected_version,
        )
    }
}
