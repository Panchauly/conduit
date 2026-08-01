//! Phase 8: capability-safe projection validation (routing, mappings, adapters).

use core::fmt;
use std::collections::{HashMap, HashSet};

use crate::adapter::document::mapping::DocumentMapping;
use crate::adapter::sql::mapping::SqlMapping;
use crate::routing::AdapterId;
use crate::runtime::config::{AdapterCapability, AdapterConfig, ConduitConfig};
use crate::runtime::dependency_graph::{
    adapter_metadata_map, execution_order_for_routed, AdapterExecutionMeta, DependencyOrderError,
};

// ---------------------------------------------------------------------------
// Issues & report
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum ValidationIssue {
    InvalidSqlMapping {
        event: String,
        reason: String,
    },
    InvalidDocumentMapping {
        event: String,
        reason: String,
    },
    UnknownAdapter {
        event_type: String,
        adapter_id: String,
    },
    MissingSqlMapping {
        event_type: String,
        adapter_id: String,
    },
    MissingDocumentMapping {
        event_type: String,
        adapter_id: String,
    },
    CapabilityMismatch {
        event_type: String,
        adapter_id: String,
        missing: Vec<AdapterCapability>,
    },
    /// SQL mapping exists but event type is not in routing.
    UnroutedSqlMapping {
        event: String,
    },
    /// Event is routed but no Sqlite adapter on the route (SQL mapping unreachable).
    SqlMappingNoSqlTarget {
        event: String,
    },
    UnroutedDocumentMapping {
        event: String,
    },
    DocumentMappingNoFileTarget {
        event: String,
    },
    /// HashMap key must match `event` field (routing and lookups use the key).
    MappingKeyMismatch {
        map_key: String,
        event_field: String,
        kind: &'static str,
    },
    /// Same adapter id listed more than once for one event type.
    DuplicateRoutingTarget {
        event_type: String,
        adapter_id: String,
    },
    /// Config lists the same adapter id more than once (defense in depth vs `ConduitConfig::validate`).
    DuplicateAdapterId {
        adapter_id: String,
    },
    /// Routed adapter depends on another adapter not on this event's route.
    DependencyNotOnRoute {
        event_type: String,
        adapter_id: String,
        dependency: String,
    },
    /// Cyclic depends_on among adapters routed for this event (`adapters` sorted by id).
    DependencyCycle {
        event_type: String,
        adapters: Vec<AdapterId>,
    },
}

impl fmt::Display for ValidationIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ValidationIssue::InvalidSqlMapping { event, reason } => {
                write!(f, "invalid SQL mapping for event {:?}: {}", event, reason)
            }
            ValidationIssue::InvalidDocumentMapping { event, reason } => {
                write!(f, "invalid document mapping for event {:?}: {}", event, reason)
            }
            ValidationIssue::UnknownAdapter {
                event_type,
                adapter_id,
            } => {
                write!(
                    f,
                    "routing: event {:?} references unknown adapter {:?}",
                    event_type, adapter_id
                )
            }
            ValidationIssue::MissingSqlMapping {
                event_type,
                adapter_id,
            } => {
                write!(
                    f,
                    "routing: event {:?} targets sql adapter {:?} but no SQL mapping exists for that event",
                    event_type, adapter_id
                )
            }
            ValidationIssue::MissingDocumentMapping {
                event_type,
                adapter_id,
            } => {
                write!(
                    f,
                    "routing: event {:?} targets document adapter {:?} but no document mapping exists for that event",
                    event_type, adapter_id
                )
            }
            ValidationIssue::CapabilityMismatch {
                event_type,
                adapter_id,
                missing,
            } => {
                let caps: Vec<_> = missing.iter().map(cap_label).collect();
                write!(
                    f,
                    "routing: event {:?} adapter {:?} lacks required capabilities: {}",
                    event_type,
                    adapter_id,
                    caps.join(", ")
                )
            }
            ValidationIssue::UnroutedSqlMapping { event } => {
                write!(
                    f,
                    "SQL mapping for event {:?} is never routed (add route or remove mapping)",
                    event
                )
            }
            ValidationIssue::SqlMappingNoSqlTarget { event } => {
                write!(
                    f,
                    "SQL mapping for event {:?} has no sqlite adapter on its route",
                    event
                )
            }
            ValidationIssue::UnroutedDocumentMapping { event } => {
                write!(
                    f,
                    "document mapping for event {:?} is never routed",
                    event
                )
            }
            ValidationIssue::DocumentMappingNoFileTarget { event } => {
                write!(
                    f,
                    "document mapping for event {:?} has no file adapter on its route",
                    event
                )
            }
            ValidationIssue::MappingKeyMismatch {
                map_key,
                event_field,
                kind,
            } => {
                write!(
                    f,
                    "{} mapping: map key {:?} must match event field {:?}",
                    kind, map_key, event_field
                )
            }
            ValidationIssue::DuplicateRoutingTarget {
                event_type,
                adapter_id,
            } => {
                write!(
                    f,
                    "routing: event {:?} lists adapter {:?} more than once",
                    event_type, adapter_id
                )
            }
            ValidationIssue::DuplicateAdapterId { adapter_id } => {
                write!(
                    f,
                    "config: duplicate adapter id {:?}",
                    adapter_id
                )
            }
            ValidationIssue::DependencyNotOnRoute {
                event_type,
                adapter_id,
                dependency,
            } => {
                write!(
                    f,
                    "routing: event {:?} adapter {:?} depends on {:?} which is not on this route",
                    event_type, adapter_id, dependency
                )
            }
            ValidationIssue::DependencyCycle {
                event_type,
                adapters,
            } => {
                write!(
                    f,
                    "routing: event {:?} has cyclic adapter dependencies among {:?}",
                    event_type,
                    adapters.join(", ")
                )
            }
        }
    }
}

fn cap_label(c: &AdapterCapability) -> &'static str {
    match c {
        AdapterCapability::Write => "write",
        AdapterCapability::Idempotent => "idempotent",
        AdapterCapability::Upsert => "upsert",
        AdapterCapability::Transactions => "transactions",
    }
}

#[derive(Debug, Clone, Default)]
pub struct ValidationReport(pub Vec<ValidationIssue>);

impl ValidationReport {
    fn push(&mut self, issue: ValidationIssue) {
        self.0.push(issue);
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn into_result(self) -> Result<(), Self> {
        if self.is_empty() {
            Ok(())
        } else {
            Err(self)
        }
    }
}

impl fmt::Display for ValidationReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.is_empty() {
            return write!(f, "(no validation issues)");
        }
        for (i, issue) in self.0.iter().enumerate() {
            if i > 0 {
                writeln!(f)?;
            }
            write!(f, "{}", issue)?;
        }
        Ok(())
    }
}

impl std::error::Error for ValidationReport {}

// ---------------------------------------------------------------------------
// Validation logic
// ---------------------------------------------------------------------------

/// First adapter per id; reports duplicate ids in `report`.
fn build_adapter_by_id<'a>(
    config: &'a ConduitConfig,
    report: &mut ValidationReport,
) -> HashMap<&'a str, &'a AdapterConfig> {
    use std::collections::hash_map::Entry;
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for a in &config.adapters {
        *counts.entry(a.id()).or_insert(0) += 1;
    }
    for (id, n) in &counts {
        if *n > 1 {
            report.push(ValidationIssue::DuplicateAdapterId {
                adapter_id: (*id).to_string(),
            });
        }
    }
    let mut map: HashMap<&'a str, &'a AdapterConfig> = HashMap::new();
    for a in &config.adapters {
        match map.entry(a.id()) {
            Entry::Vacant(v) => {
                v.insert(a);
            }
            Entry::Occupied(_) => {}
        }
    }
    map
}

fn is_sqlite(a: &AdapterConfig) -> bool {
    matches!(a, AdapterConfig::Sqlite(_))
}

fn is_file(a: &AdapterConfig) -> bool {
    matches!(a, AdapterConfig::File(_))
}

/// When unset or empty, adapter is treated as declaring only `Write`.
fn effective_capabilities(adapter: &AdapterConfig) -> HashSet<AdapterCapability> {
    match adapter.capabilities() {
        None | Some([]) => [AdapterCapability::Write].into_iter().collect(),
        Some(v) => v.iter().copied().collect(),
    }
}

fn document_template_empty(doc: &serde_json::Value) -> bool {
    doc.is_null() || doc.as_object().is_some_and(|o| o.is_empty()) || doc.as_array().is_some_and(|a| a.is_empty())
}

fn validate_route_dependencies(
    event_type: &str,
    adapter_ids: &[AdapterId],
    adapters_by_id: &HashMap<&str, &AdapterConfig>,
    adapter_meta: &HashMap<AdapterId, AdapterExecutionMeta>,
    report: &mut ValidationReport,
) {
    if !adapter_ids
        .iter()
        .all(|id| adapters_by_id.contains_key(id.as_str()))
    {
        return;
    }
    let routed_set: HashSet<_> = adapter_ids.iter().cloned().collect();
    for aid in &routed_set {
        let ac = adapters_by_id[aid.as_str()];
        for d in ac.depends_on() {
            if !routed_set.contains(d) {
                report.push(ValidationIssue::DependencyNotOnRoute {
                    event_type: event_type.to_string(),
                    adapter_id: aid.clone(),
                    dependency: d.clone(),
                });
            }
        }
    }
    match execution_order_for_routed(adapter_ids, adapter_meta) {
        Err(DependencyOrderError::Cycle { adapters }) => {
            report.push(ValidationIssue::DependencyCycle {
                event_type: event_type.to_string(),
                adapters,
            });
        }
        Err(DependencyOrderError::MissingAdapterMeta { .. }) => {}
        Ok(_) => {}
    }
}

/// Validate routing + dependency closure + acyclicity for one event type; returns execution order.
pub fn validate_routing_and_dependencies_for_event_type(
    config: &ConduitConfig,
    routing: &HashMap<String, Vec<AdapterId>>,
    event_type: &str,
) -> Result<Vec<AdapterId>, ValidationReport> {
    let mut report = ValidationReport::default();
    let adapters_by_id = build_adapter_by_id(config, &mut report);
    let adapter_meta = adapter_metadata_map(config);

    let Some(adapter_ids) = routing.get(event_type) else {
        return if report.is_empty() {
            Ok(Vec::new())
        } else {
            Err(report)
        };
    };

    let mut seen_route = HashSet::new();
    for adapter_id in adapter_ids {
        if !seen_route.insert(adapter_id.as_str()) {
            report.push(ValidationIssue::DuplicateRoutingTarget {
                event_type: event_type.to_string(),
                adapter_id: adapter_id.clone(),
            });
        }
    }
    for adapter_id in adapter_ids {
        if !adapters_by_id.contains_key(adapter_id.as_str()) {
            report.push(ValidationIssue::UnknownAdapter {
                event_type: event_type.to_string(),
                adapter_id: adapter_id.clone(),
            });
        }
    }

    validate_route_dependencies(
        event_type,
        adapter_ids,
        &adapters_by_id,
        &adapter_meta,
        &mut report,
    );

    if !report.is_empty() {
        return Err(report);
    }

    match execution_order_for_routed(adapter_ids, &adapter_meta) {
        Ok(order) => Ok(order),
        Err(DependencyOrderError::Cycle { adapters }) => {
            report.push(ValidationIssue::DependencyCycle {
                event_type: event_type.to_string(),
                adapters,
            });
            Err(report)
        }
        Err(DependencyOrderError::MissingAdapterMeta { adapter_id }) => {
            report.push(ValidationIssue::UnknownAdapter {
                event_type: event_type.to_string(),
                adapter_id,
            });
            Err(report)
        }
    }
}

/// Full Phase 8 validation: routing ↔ adapters ↔ mappings ↔ capabilities, plus orphan mappings.
pub fn validate_projection_config(
    config: &ConduitConfig,
    routing: &HashMap<String, Vec<AdapterId>>,
    sql: &HashMap<String, SqlMapping>,
    doc: &HashMap<String, DocumentMapping>,
) -> Result<(), ValidationReport> {
    let mut report = ValidationReport::default();
    let adapters_by_id = build_adapter_by_id(config, &mut report);
    let adapter_meta = adapter_metadata_map(config);

    for (event_type, adapter_ids) in routing {
        let mut seen_route: HashSet<&str> = HashSet::new();
        for adapter_id in adapter_ids {
            if !seen_route.insert(adapter_id.as_str()) {
                report.push(ValidationIssue::DuplicateRoutingTarget {
                    event_type: event_type.clone(),
                    adapter_id: adapter_id.clone(),
                });
            }
        }
    }

    for (event_type, adapter_ids) in routing {
        validate_route_dependencies(
            event_type,
            adapter_ids,
            &adapters_by_id,
            &adapter_meta,
            &mut report,
        );
    }

    for (key, m) in sql {
        if key != m.event.as_str() {
            report.push(ValidationIssue::MappingKeyMismatch {
                map_key: key.clone(),
                event_field: m.event.clone(),
                kind: "SQL",
            });
        }
        if m.event.trim().is_empty() {
            report.push(ValidationIssue::InvalidSqlMapping {
                event: key.clone(),
                reason: "empty event".into(),
            });
        } else if m.table.trim().is_empty() {
            report.push(ValidationIssue::InvalidSqlMapping {
                event: key.clone(),
                reason: "empty table".into(),
            });
        } else if m.primary_key.trim().is_empty() {
            report.push(ValidationIssue::InvalidSqlMapping {
                event: key.clone(),
                reason: "empty primary_key".into(),
            });
        } else if m.columns.is_empty() {
            report.push(ValidationIssue::InvalidSqlMapping {
                event: key.clone(),
                reason: "columns must be non-empty".into(),
            });
        }
    }

    for (key, m) in doc {
        if key != m.event.as_str() {
            report.push(ValidationIssue::MappingKeyMismatch {
                map_key: key.clone(),
                event_field: m.event.clone(),
                kind: "document",
            });
        }
        if m.event.trim().is_empty() {
            report.push(ValidationIssue::InvalidDocumentMapping {
                event: key.clone(),
                reason: "empty event".into(),
            });
        } else if m.collection.trim().is_empty() {
            report.push(ValidationIssue::InvalidDocumentMapping {
                event: key.clone(),
                reason: "empty collection".into(),
            });
        } else if document_template_empty(&m.document) {
            report.push(ValidationIssue::InvalidDocumentMapping {
                event: key.clone(),
                reason: "document template must not be empty (null, {}, or [])".into(),
            });
        }
    }

    for (event_type, adapter_ids) in routing {
        for adapter_id in adapter_ids {
            let Some(ac) = adapters_by_id.get(adapter_id.as_str()).copied() else {
                report.push(ValidationIssue::UnknownAdapter {
                    event_type: event_type.clone(),
                    adapter_id: adapter_id.clone(),
                });
                continue;
            };

            if is_sqlite(ac) {
                let Some(sql_map) = sql.get(event_type) else {
                    report.push(ValidationIssue::MissingSqlMapping {
                        event_type: event_type.clone(),
                        adapter_id: adapter_id.clone(),
                    });
                    continue;
                };
                let eff = effective_capabilities(ac);
                let missing: Vec<_> = sql_map
                    .requires_capabilities
                    .iter()
                    .copied()
                    .filter(|c| !eff.contains(c))
                    .collect();
                if !missing.is_empty() {
                    report.push(ValidationIssue::CapabilityMismatch {
                        event_type: event_type.clone(),
                        adapter_id: adapter_id.clone(),
                        missing,
                    });
                }
            } else if is_file(ac) {
                let Some(doc_map) = doc.get(event_type) else {
                    report.push(ValidationIssue::MissingDocumentMapping {
                        event_type: event_type.clone(),
                        adapter_id: adapter_id.clone(),
                    });
                    continue;
                };
                let eff = effective_capabilities(ac);
                let missing: Vec<_> = doc_map
                    .requires_capabilities
                    .iter()
                    .copied()
                    .filter(|c| !eff.contains(c))
                    .collect();
                if !missing.is_empty() {
                    report.push(ValidationIssue::CapabilityMismatch {
                        event_type: event_type.clone(),
                        adapter_id: adapter_id.clone(),
                        missing,
                    });
                }
            }
        }
    }

    for event in sql.keys() {
        let Some(ids) = routing.get(event) else {
            report.push(ValidationIssue::UnroutedSqlMapping {
                event: event.clone(),
            });
            continue;
        };
        let has_sqlite = ids.iter().any(|id| {
            adapters_by_id
                .get(id.as_str())
                .copied()
                .map(is_sqlite)
                .unwrap_or(false)
        });
        if !has_sqlite {
            report.push(ValidationIssue::SqlMappingNoSqlTarget {
                event: event.clone(),
            });
        }
    }

    for event in doc.keys() {
        let Some(ids) = routing.get(event) else {
            report.push(ValidationIssue::UnroutedDocumentMapping {
                event: event.clone(),
            });
            continue;
        };
        let has_file = ids.iter().any(|id| {
            adapters_by_id
                .get(id.as_str())
                .copied()
                .map(is_file)
                .unwrap_or(false)
        });
        if !has_file {
            report.push(ValidationIssue::DocumentMappingNoFileTarget {
                event: event.clone(),
            });
        }
    }

    report.into_result()
}

/// Validate that every adapter id appearing anywhere in routing exists in config.
pub fn validate_routing_adapter_ids(
    config: &ConduitConfig,
    routing: &HashMap<String, Vec<AdapterId>>,
) -> Result<(), ValidationReport> {
    let mut report = ValidationReport::default();
    let adapters_by_id = build_adapter_by_id(config, &mut report);
    for (event_type, adapter_ids) in routing {
        for adapter_id in adapter_ids {
            if !adapters_by_id.contains_key(adapter_id.as_str()) {
                report.push(ValidationIssue::UnknownAdapter {
                    event_type: event_type.clone(),
                    adapter_id: adapter_id.clone(),
                });
            }
        }
    }
    report.into_result()
}

/// Adapters returned by routing for this event type must exist in config (e.g. `explain` without full mappings).
pub fn validate_routing_for_event_type(
    config: &ConduitConfig,
    routing: &HashMap<String, Vec<AdapterId>>,
    event_type: &str,
) -> Result<(), ValidationReport> {
    let mut report = ValidationReport::default();
    let adapters_by_id = build_adapter_by_id(config, &mut report);
    let Some(adapter_ids) = routing.get(event_type) else {
        return report.into_result();
    };
    for adapter_id in adapter_ids {
        if !adapters_by_id.contains_key(adapter_id.as_str()) {
            report.push(ValidationIssue::UnknownAdapter {
                event_type: event_type.to_string(),
                adapter_id: adapter_id.clone(),
            });
        }
    }
    report.into_result()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_template_empty_detects_null_object_array() {
        assert!(document_template_empty(&serde_json::Value::Null));
        assert!(document_template_empty(&serde_json::json!({})));
        assert!(document_template_empty(&serde_json::json!([])));
        assert!(!document_template_empty(&serde_json::json!({ "a": 1 })));
    }
}
