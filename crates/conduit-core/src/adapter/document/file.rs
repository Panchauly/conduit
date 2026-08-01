use std::path::PathBuf;
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

    // fn guard_dir(&self) -> PathBuf {
    //     self.root.join(".conduit").join("events")
    // }

    // fn guard_file(&self, event_id: &str) -> PathBuf {
    //     self.guard_dir().join(format!("{}.done", event_id))
    // }
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
        let guard = self
            .root
            .join(".conduit")
            .join("events")
            .join(format!("{}.done", event.event_id));

        // --------------------------------------------------
        // PHASE 4: Idempotency check (BEFORE write)
        // --------------------------------------------------
        if guard.exists() {
            return AdapterResult::skipped(
                self.id.clone(),
                StorageKind::Document,
                "event already processed".to_string(),
            );
        }

        // Build document projection (upcasting the payload to the mapping's target version first)
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

        // --------------------------------------------------
        // PHASE 3 LOGIC — UNCHANGED & VALID
        // --------------------------------------------------
        let write_result: Result<(), Box<dyn std::error::Error>> = (|| {
            let out_path = self
                .root
                .join(&event.event_type)
                .join(format!("{}.json", event.event_id));

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
        // PHASE 4: Record idempotency guard (AFTER write)
        // --------------------------------------------------
        let Some(guard_parent) = guard.parent() else {
            return self.failure(AdapterError::WriteFailed(
                "idempotency guard path has no parent directory".to_string(),
            ));
        };
        if let Err(e) =
            std::fs::create_dir_all(guard_parent).and_then(|_| std::fs::write(&guard, "ok"))
        {
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
