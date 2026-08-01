use std::path::PathBuf;

use crate::{
    adapter::document::runtime::DocumentRuntimeBuilder,
    adapter::{AdapterError, AdapterResult, StorageAdapter},
    event::Event,
    routing::StorageKind,
};

/// File-based document adapter (Phase 4: idempotent)
pub struct FileDocumentAdapter {
    id: String,
    priority: u32,
    root: PathBuf,
    builder: DocumentRuntimeBuilder,
}

impl FileDocumentAdapter {
    pub fn new(id: String, root: PathBuf, priority: u32, builder: DocumentRuntimeBuilder) -> Self {
        Self {
            id,
            root,
            priority,
            builder,
        }
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
            return AdapterResult {
                adapter_id: self.id.clone(),
                kind: StorageKind::Document,
                success: true,
                error: Some(AdapterError::Skipped("event already processed".to_string())),
            };
        }

        // --------------------------------------------------
        // PHASE 3 LOGIC — UNCHANGED & VALID
        // --------------------------------------------------
        let write_result: Result<(), Box<dyn std::error::Error>> = (|| {
            // Build document (pure projection)
            let document = self.builder.build(event)?;

            // Collection derived deterministically
            let out_path = self
                .root
                .join(&event.event_type)
                .join(format!("{}.json", event.event_id));

            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent)?;
            }

            std::fs::write(&out_path, serde_json::to_string_pretty(&document)?)?;

            Ok(())
        })();

        if let Err(e) = write_result {
            return AdapterResult {
                adapter_id: self.id.clone(),
                kind: StorageKind::Document,
                success: false,
                error: Some(AdapterError::WriteFailed(e.to_string())),
            };
        }

        // --------------------------------------------------
        // PHASE 4: Record idempotency guard (AFTER write)
        // --------------------------------------------------
        let Some(guard_parent) = guard.parent() else {
            return AdapterResult {
                adapter_id: self.id.clone(),
                kind: StorageKind::Document,
                success: false,
                error: Some(AdapterError::WriteFailed(
                    "idempotency guard path has no parent directory".to_string(),
                )),
            };
        };
        if let Err(e) =
            std::fs::create_dir_all(guard_parent).and_then(|_| std::fs::write(&guard, "ok"))
        {
            return AdapterResult {
                adapter_id: self.id.clone(),
                kind: StorageKind::Document,
                success: false,
                error: Some(AdapterError::WriteFailed(e.to_string())),
            };
        }

        AdapterResult {
            adapter_id: self.id.clone(),
            kind: StorageKind::Document,
            success: true,
            error: None,
        }
    }
}
