use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::{
    adapter::graph::adapter::GraphError,
    adapter::graph::mapping::{ResolvedRecord, canonical_edge_key, encode_edge_key},
    adapter::graph::runtime::GraphRuntimeBuilder,
    adapter::{
        AdapterError, AdapterResult, GuardState, OnExisting, SkipReason, StorageAdapter,
        WriteDecision, decide,
    },
    event::Event,
    routing::StorageKind,
    runtime::config::MigrationPolicy,
    upcast::UpcasterRegistry,
};

/// Idempotency guard sidecar for one graph record (node or edge) — structurally
/// identical to the Document / KV / Phase 15 guard: the default facet is in the
/// flat fields (a non-faceted record round-trips unchanged), named-facet lanes
/// (nodes only) live in `facets`.
#[derive(Debug, Default, Serialize, Deserialize)]
struct GraphGuard {
    #[serde(default)]
    last_sequence: u64,
    #[serde(default)]
    last_event_id: String,
    #[serde(default)]
    deleted: bool,
    #[serde(default)]
    permanent: bool,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    facets: BTreeMap<String, GraphFacetGuard>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct GraphFacetGuard {
    last_sequence: u64,
    last_event_id: String,
    #[serde(default)]
    deleted: bool,
    #[serde(default)]
    permanent: bool,
}

impl GraphGuard {
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

/// One incident edge recorded against a node (Phase 16.2). Carries both
/// endpoints so the node-delete detach cascade can drop the edge from the
/// *other* endpoint's index too.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct IncidentRef {
    edge_type: String,
    /// Filesystem-safe encoded canonical edge key.
    key: String,
    from: String,
    to: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct IncidentIndex {
    #[serde(default)]
    edges: Vec<IncidentRef>,
}

/// File-backed directed property graph (Phase 16) — the fourth `StorageAdapter`.
/// Nodes and edges are *separately keyed records*, each an independent Phase
/// 12/13 sequence-gated state machine driven by the same `decide()` as every
/// prior adapter. Nodes are entities (Phase 15 facets apply); edges are not
/// faceted.
pub struct GraphStore {
    id: String,
    priority: u32,
    root: PathBuf,
    builder: GraphRuntimeBuilder,
    upcasters: Arc<UpcasterRegistry>,
    migration_policy: MigrationPolicy,
}

impl GraphStore {
    pub fn new(
        id: String,
        root: PathBuf,
        priority: u32,
        builder: GraphRuntimeBuilder,
        upcasters: Arc<UpcasterRegistry>,
        migration_policy: MigrationPolicy,
    ) -> Self {
        Self {
            id,
            priority,
            root,
            builder,
            upcasters,
            migration_policy,
        }
    }

    fn failure(&self, error: AdapterError) -> AdapterResult {
        AdapterResult::failure(self.id.clone(), StorageKind::Graph, error)
    }

    fn node_value_path(&self, label: &str, node_id: &str) -> PathBuf {
        self.root
            .join("nodes")
            .join(label)
            .join(format!("{node_id}.json"))
    }

    fn node_guard_path(&self, label: &str, node_id: &str) -> PathBuf {
        self.root
            .join(".conduit")
            .join("guard")
            .join("nodes")
            .join(label)
            .join(format!("{node_id}.json"))
    }

    fn edge_value_path(&self, edge_type: &str, encoded_key: &str) -> PathBuf {
        self.root
            .join("edges")
            .join(edge_type)
            .join(format!("{encoded_key}.json"))
    }

    fn edge_guard_path(&self, edge_type: &str, encoded_key: &str) -> PathBuf {
        self.root
            .join(".conduit")
            .join("guard")
            .join("edges")
            .join(edge_type)
            .join(format!("{encoded_key}.json"))
    }

    fn incident_path(&self, node_id: &str) -> PathBuf {
        self.root
            .join(".conduit")
            .join("incident")
            .join(format!("{node_id}.json"))
    }

    fn read_guard(&self, path: &Path) -> std::io::Result<Option<GraphGuard>> {
        match std::fs::read_to_string(path) {
            Ok(content) => {
                let parsed: GraphGuard = serde_json::from_str(&content)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
                Ok(Some(parsed))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn read_incident(&self, node_id: &str) -> std::io::Result<IncidentIndex> {
        match std::fs::read_to_string(self.incident_path(node_id)) {
            Ok(content) => serde_json::from_str(&content)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(IncidentIndex::default()),
            Err(e) => Err(e),
        }
    }

    /// Write JSON via write-to-temp-then-rename (Phase 11.3 pattern).
    fn commit_json<T: Serialize>(&self, path: &Path, value: &T) -> std::io::Result<()> {
        let parent = path.parent().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "path has no parent directory",
            )
        })?;
        std::fs::create_dir_all(parent)?;
        let tmp_name = format!(
            ".{}.tmp-{}",
            path.file_name().and_then(|n| n.to_str()).unwrap_or("f"),
            std::process::id()
        );
        let tmp = parent.join(tmp_name);
        let content = serde_json::to_string_pretty(value)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(&tmp, content)?;
        std::fs::rename(&tmp, path)
    }

    fn remove_if_exists(&self, path: &Path) -> std::io::Result<()> {
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }

    fn add_incident(&self, node_id: &str, r: &IncidentRef) -> std::io::Result<()> {
        let mut idx = self.read_incident(node_id)?;
        if !idx
            .edges
            .iter()
            .any(|e| e.edge_type == r.edge_type && e.key == r.key)
        {
            idx.edges.push(r.clone());
            self.commit_json(&self.incident_path(node_id), &idx)?;
        }
        Ok(())
    }

    fn drop_incident(&self, node_id: &str, edge_type: &str, key: &str) -> std::io::Result<()> {
        let mut idx = self.read_incident(node_id)?;
        let before = idx.edges.len();
        idx.edges
            .retain(|e| !(e.edge_type == edge_type && e.key == key));
        if idx.edges.len() != before {
            if idx.edges.is_empty() {
                self.remove_if_exists(&self.incident_path(node_id))?;
            } else {
                self.commit_json(&self.incident_path(node_id), &idx)?;
            }
        }
        Ok(())
    }

    fn skipped(&self, reason: SkipReason, p: &GraphProjectionMeta) -> AdapterResult {
        AdapterResult::skipped_versioned(
            self.id.clone(),
            StorageKind::Graph,
            reason,
            p.source_version,
            p.projected_version,
        )
    }

    fn outcome(&self, decision: WriteDecision, p: &GraphProjectionMeta) -> AdapterResult {
        match decision {
            WriteDecision::Insert => AdapterResult::created_versioned(
                self.id.clone(),
                StorageKind::Graph,
                p.source_version,
                p.projected_version,
            ),
            WriteDecision::Update => AdapterResult::updated_versioned(
                self.id.clone(),
                StorageKind::Graph,
                p.source_version,
                p.projected_version,
            ),
            WriteDecision::Delete => AdapterResult::deleted_versioned(
                self.id.clone(),
                StorageKind::Graph,
                p.source_version,
                p.projected_version,
            ),
            _ => self.failure(AdapterError::WriteFailed(
                "unreachable: skip decision reached the write path".to_string(),
            )),
        }
    }
}

/// Just the version metadata, threaded to the result helpers.
struct GraphProjectionMeta {
    source_version: u32,
    projected_version: u32,
}

impl StorageAdapter for GraphStore {
    fn kind(&self) -> StorageKind {
        StorageKind::Graph
    }

    fn id(&self) -> &str {
        &self.id
    }

    fn priority(&self) -> u32 {
        self.priority
    }

    fn handle(&self, event: &Event) -> AdapterResult {
        let projection = match self.builder.build(event, &self.upcasters) {
            Ok(p) => p,
            Err(GraphError::UnsupportedVersion {
                from_version,
                to_version,
                reason,
                ..
            }) => {
                return match self.migration_policy {
                    MigrationPolicy::Strict => AdapterResult::unsupported_version(
                        self.id.clone(),
                        StorageKind::Graph,
                        reason,
                        from_version,
                        to_version,
                    ),
                    MigrationPolicy::IgnoreUnmatched => AdapterResult::skipped_versioned(
                        self.id.clone(),
                        StorageKind::Graph,
                        SkipReason::UnsupportedVersion,
                        from_version,
                        to_version,
                    ),
                };
            }
            Err(e) => return self.failure(AdapterError::WriteFailed(e.to_string())),
        };

        let meta = GraphProjectionMeta {
            source_version: projection.source_version,
            projected_version: projection.projected_version,
        };

        match &projection.record {
            ResolvedRecord::Node {
                label,
                node_id,
                properties,
            } => self.handle_node(
                event,
                &projection,
                &meta,
                label,
                node_id,
                properties.clone(),
            ),
            ResolvedRecord::Edge {
                edge_type,
                from,
                to,
                discriminator,
                properties,
            } => self.handle_edge(
                event,
                &projection,
                &meta,
                edge_type,
                from,
                to,
                discriminator.as_deref(),
                properties.clone(),
            ),
        }
    }
}

impl GraphStore {
    #[allow(clippy::too_many_arguments)]
    fn handle_node(
        &self,
        event: &Event,
        projection: &crate::adapter::graph::runtime::GraphProjection,
        meta: &GraphProjectionMeta,
        label: &str,
        node_id: &str,
        properties: serde_json::Value,
    ) -> AdapterResult {
        let facet = projection.facet.clone();
        let is_named_facet = !facet.is_empty();
        let guard_path = self.node_guard_path(label, node_id);
        let value_path = self.node_value_path(label, node_id);

        let mut stored_guard = match self.read_guard(&guard_path) {
            Ok(g) => g,
            Err(e) => return self.failure(AdapterError::WriteFailed(e.to_string())),
        };

        if is_named_facet {
            let present = stored_guard
                .as_ref()
                .and_then(|g| g.lane_state(""))
                .is_some_and(|s| !s.deleted);
            if !present {
                return self.skipped(SkipReason::EntityAbsent, meta);
            }
        }

        let stored = stored_guard.as_ref().and_then(|g| g.lane_state(&facet));
        let effective_mode = if is_named_facet {
            OnExisting::Replace
        } else {
            projection.on_existing
        };
        let decision = decide(projection.operation, effective_mode, stored, event.sequence);
        let was_tombstoned_default = !is_named_facet && stored.is_some_and(|s| s.deleted);

        match decision {
            WriteDecision::SkipIdempotent => {
                return self.skipped(SkipReason::AlreadyProjected, meta);
            }
            WriteDecision::SkipStale => return self.skipped(SkipReason::StaleSequence, meta),
            WriteDecision::SkipTombstoned => return self.skipped(SkipReason::Tombstoned, meta),
            WriteDecision::SkipAlreadyDeleted => {
                let mut g = stored_guard.take().unwrap_or_default();
                g.last_sequence = event.sequence;
                g.last_event_id = event.event_id.clone();
                g.deleted = true;
                if let Err(e) = self.commit_json(&guard_path, &g) {
                    return self.failure(AdapterError::WriteFailed(e.to_string()));
                }
                return self.skipped(SkipReason::AlreadyDeleted, meta);
            }
            WriteDecision::Insert | WriteDecision::Update | WriteDecision::Delete => {}
        }

        let write: Result<(), Box<dyn std::error::Error>> = (|| {
            if decision == WriteDecision::Delete {
                // DETACH DELETE (Phase 16.4): remove the node, then every
                // incident edge, tombstoning each at this event's sequence.
                self.remove_if_exists(&value_path)?;
                let incident = self.read_incident(node_id)?;
                for r in &incident.edges {
                    self.remove_if_exists(&self.edge_value_path(&r.edge_type, &r.key))?;
                    let egp = self.edge_guard_path(&r.edge_type, &r.key);
                    let mut eg = self.read_guard(&egp)?.unwrap_or_default();
                    eg.last_sequence = event.sequence;
                    eg.last_event_id = event.event_id.clone();
                    eg.deleted = true;
                    eg.permanent = projection.permanent;
                    self.commit_json(&egp, &eg)?;
                    // Drop the edge from the *other* endpoint's index.
                    for other in [&r.from, &r.to] {
                        if other.as_str() != node_id {
                            self.drop_incident(other, &r.edge_type, &r.key)?;
                        }
                    }
                }
                self.remove_if_exists(&self.incident_path(node_id))?;
            } else if is_named_facet {
                // Phase 15.4: shallow-merge the facet's top-level keys.
                let facet_obj = properties
                    .as_object()
                    .ok_or("node facet mapping must resolve to a JSON object")?;
                let mut existing = match std::fs::read_to_string(&value_path) {
                    Ok(s) => serde_json::from_str::<serde_json::Value>(&s)
                        .unwrap_or_else(|_| serde_json::Value::Object(Default::default())),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        serde_json::Value::Object(Default::default())
                    }
                    Err(e) => return Err(e.into()),
                };
                let obj = existing
                    .as_object_mut()
                    .ok_or("existing node document is not a JSON object")?;
                for (k, v) in facet_obj {
                    obj.insert(k.clone(), v.clone());
                }
                self.commit_json(&value_path, &existing)?;
            } else {
                self.commit_json(&value_path, &properties)?;
            }
            Ok(())
        })();
        if let Err(e) = write {
            return self.failure(AdapterError::WriteFailed(e.to_string()));
        }

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
        if let Err(e) = self.commit_json(&guard_path, &g) {
            return self.failure(AdapterError::WriteFailed(e.to_string()));
        }

        self.outcome(decision, meta)
    }

    #[allow(clippy::too_many_arguments)]
    fn handle_edge(
        &self,
        event: &Event,
        projection: &crate::adapter::graph::runtime::GraphProjection,
        meta: &GraphProjectionMeta,
        edge_type: &str,
        from: &str,
        to: &str,
        discriminator: Option<&str>,
        properties: serde_json::Value,
    ) -> AdapterResult {
        let canonical = canonical_edge_key(edge_type, from, to, discriminator);
        let key = encode_edge_key(&canonical);
        let guard_path = self.edge_guard_path(edge_type, &key);
        let value_path = self.edge_value_path(edge_type, &key);

        let mut stored_guard = match self.read_guard(&guard_path) {
            Ok(g) => g,
            Err(e) => return self.failure(AdapterError::WriteFailed(e.to_string())),
        };
        let stored = stored_guard.as_ref().and_then(|g| g.lane_state(""));
        let decision = decide(
            projection.operation,
            projection.on_existing,
            stored,
            event.sequence,
        );

        match decision {
            WriteDecision::SkipIdempotent => {
                return self.skipped(SkipReason::AlreadyProjected, meta);
            }
            WriteDecision::SkipStale => return self.skipped(SkipReason::StaleSequence, meta),
            WriteDecision::SkipTombstoned => return self.skipped(SkipReason::Tombstoned, meta),
            WriteDecision::SkipAlreadyDeleted => {
                let mut g = stored_guard.take().unwrap_or_default();
                g.last_sequence = event.sequence;
                g.last_event_id = event.event_id.clone();
                g.deleted = true;
                if let Err(e) = self.commit_json(&guard_path, &g) {
                    return self.failure(AdapterError::WriteFailed(e.to_string()));
                }
                return self.skipped(SkipReason::AlreadyDeleted, meta);
            }
            WriteDecision::Insert | WriteDecision::Update | WriteDecision::Delete => {}
        }

        // Phase 16.4: an edge may not attach to an explicitly-tombstoned node
        // (DETACH DELETE consequence). An endpoint that was *never* projected
        // is fine — the file adapter is permissive about dangling ids (16.3).
        if decision != WriteDecision::Delete {
            for endpoint in dedup_pair(from, to) {
                for label_dir in read_label_dirs(&self.root) {
                    let gp = self.node_guard_path(&label_dir, endpoint);
                    match self.read_guard(&gp) {
                        Ok(Some(g)) if g.deleted => {
                            return self.skipped(SkipReason::EntityAbsent, meta);
                        }
                        Ok(_) => {}
                        Err(e) => return self.failure(AdapterError::WriteFailed(e.to_string())),
                    }
                }
            }
        }

        let write: Result<(), Box<dyn std::error::Error>> = (|| {
            if decision == WriteDecision::Delete {
                self.remove_if_exists(&value_path)?;
                for endpoint in dedup_pair(from, to) {
                    self.drop_incident(endpoint, edge_type, &key)?;
                }
            } else {
                self.commit_json(&value_path, &properties)?;
                let r = IncidentRef {
                    edge_type: edge_type.to_string(),
                    key: key.clone(),
                    from: from.to_string(),
                    to: to.to_string(),
                };
                for endpoint in dedup_pair(from, to) {
                    self.add_incident(endpoint, &r)?;
                }
            }
            Ok(())
        })();
        if let Err(e) = write {
            return self.failure(AdapterError::WriteFailed(e.to_string()));
        }

        let mut g = stored_guard.take().unwrap_or_default();
        g.last_sequence = event.sequence;
        g.last_event_id = event.event_id.clone();
        g.deleted = decision == WriteDecision::Delete;
        g.permanent = decision == WriteDecision::Delete && projection.permanent;
        if let Err(e) = self.commit_json(&guard_path, &g) {
            return self.failure(AdapterError::WriteFailed(e.to_string()));
        }

        self.outcome(decision, meta)
    }
}

fn dedup_pair<'a>(a: &'a str, b: &'a str) -> Vec<&'a str> {
    if a == b { vec![a] } else { vec![a, b] }
}

/// Every `label` directory under `{root}/.conduit/guard/nodes/` — the set of
/// labels a node with a given id could have been projected under. Small in
/// practice (one project rarely has many node labels); missing dir → empty.
fn read_label_dirs(root: &Path) -> Vec<String> {
    let dir = root.join(".conduit").join("guard").join("nodes");
    match std::fs::read_dir(&dir) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .filter_map(|e| e.file_name().into_string().ok())
            .collect(),
        Err(_) => Vec::new(),
    }
}
