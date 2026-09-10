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
/// adapter's `ProjectionGuard` (Phase 11.3/12.4/13.3/15.2), just named for the
/// third storage kind. The default facet stays in the flat fields (a
/// pre-Phase-15 sidecar round-trips unchanged); named-facet lanes live in
/// `facets`.
#[derive(Debug, Default, Serialize, Deserialize)]
struct KvGuard {
    #[serde(default)]
    last_sequence: u64,
    #[serde(default)]
    last_event_id: String,
    #[serde(default)]
    deleted: bool,
    #[serde(default)]
    permanent: bool,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    facets: std::collections::BTreeMap<String, KvFacetGuard>,
}

/// One named-facet lane inside a [`KvGuard`] (Phase 15.2).
#[derive(Debug, Default, Serialize, Deserialize)]
struct KvFacetGuard {
    last_sequence: u64,
    last_event_id: String,
    #[serde(default)]
    deleted: bool,
    #[serde(default)]
    permanent: bool,
}

impl KvGuard {
    /// The Phase 12/13 [`GuardState`] for one lane — flat fields for the
    /// default facet (`""`), a `facets` entry for a named one.
    fn lane_state(&self, facet: &str) -> Option<GuardState> {
        if facet.is_empty() {
            Some(GuardState {
                last_sequence: self.last_sequence,
                deleted: self.deleted,
                permanent: self.permanent,
            })
        } else {
            self.facets.get(facet).map(|f| GuardState {
                last_sequence: f.last_sequence,
                deleted: f.deleted,
                permanent: f.permanent,
            })
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

        let facet = projection.facet.clone();
        let is_named_facet = !facet.is_empty();
        let guard_path = self.guard_path(&projection.namespace, &projection.entity_key);

        // Read-decide-write, all against this one sidecar file. Known
        // limitation (Phase 12.4 non-goal, inherited): not atomic across
        // processes — the store assumes a single writer.
        let mut stored_guard = match self.read_guard(&guard_path) {
            Ok(g) => g,
            Err(e) => return self.failure(AdapterError::WriteFailed(e.to_string())),
        };

        // PHASE 15.3: entity-existence pre-check for a named facet.
        if is_named_facet {
            let entity_present = stored_guard
                .as_ref()
                .and_then(|g| g.lane_state(""))
                .is_some_and(|s| !s.deleted);
            if !entity_present {
                return AdapterResult::skipped_versioned(
                    self.id.clone(),
                    StorageKind::KeyValue,
                    SkipReason::EntityAbsent,
                    projection.source_version,
                    projection.projected_version,
                );
            }
        }

        let stored: Option<GuardState> = stored_guard.as_ref().and_then(|g| g.lane_state(&facet));

        // A named facet is always a sequence-gated partial replace of its own
        // keys; `decide()` is unchanged (Phase 15.2).
        let effective_mode = if is_named_facet {
            crate::adapter::OnExisting::Replace
        } else {
            projection.on_existing
        };
        let decision = decide(projection.operation, effective_mode, stored, event.sequence);
        let was_tombstoned_default = !is_named_facet && stored.is_some_and(|s| s.deleted);

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
                // sequence seen, not a stale one. (Default facet only.)
                let mut g = stored_guard.take().unwrap_or_default();
                g.last_sequence = event.sequence;
                g.last_event_id = event.event_id.clone();
                g.deleted = true;
                if let Err(e) = self.commit_guard(&guard_path, &g) {
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
            } else if is_named_facet {
                // Phase 15.4: shallow-merge the facet's own top-level keys into
                // the existing value; every other key is preserved.
                let facet_obj = projection
                    .value
                    .as_object()
                    .ok_or("facet mapping must resolve to a JSON object")?;
                let mut existing = match std::fs::read_to_string(&value_path) {
                    Ok(s) => serde_json::from_str::<serde_json::Value>(&s)
                        .unwrap_or_else(|_| serde_json::Value::Object(Default::default())),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        serde_json::Value::Object(Default::default())
                    }
                    Err(e) => return Err(e.into()),
                };
                let existing_obj = existing
                    .as_object_mut()
                    .ok_or("existing value is not a JSON object")?;
                for (k, v) in facet_obj {
                    existing_obj.insert(k.clone(), v.clone());
                }
                if let Some(parent) = value_path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&value_path, serde_json::to_string_pretty(&existing)?)?;
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

        // Commit the guard AFTER the write, via atomic rename, preserving lanes
        // this event didn't touch (Phase 15.2).
        let mut g = stored_guard.take().unwrap_or_default();
        if is_named_facet {
            let fg = g.facets.entry(facet.clone()).or_default();
            fg.last_sequence = event.sequence;
            fg.last_event_id = event.event_id.clone();
            fg.deleted = false;
            fg.permanent = false;
        } else {
            g.last_sequence = event.sequence;
            g.last_event_id = event.event_id.clone();
            g.deleted = decision == WriteDecision::Delete;
            g.permanent = decision == WriteDecision::Delete && projection.permanent;
            if decision == WriteDecision::Delete {
                for fg in g.facets.values_mut() {
                    fg.deleted = true;
                    fg.permanent = projection.permanent;
                    fg.last_sequence = event.sequence;
                    fg.last_event_id = event.event_id.clone();
                }
            } else if was_tombstoned_default && decision == WriteDecision::Insert {
                for fg in g.facets.values_mut() {
                    fg.deleted = false;
                    fg.permanent = false;
                }
            }
        }
        if let Err(e) = self.commit_guard(&guard_path, &g) {
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
