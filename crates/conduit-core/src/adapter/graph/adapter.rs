use std::fmt;

/// Errors related to graph projection and writing. Structurally identical to
/// `KvError` / `DocumentError` / `SqlError` — same four cases, same meaning,
/// named for the fourth storage kind (Phase 16).
#[derive(Debug)]
pub enum GraphError {
    /// No mapping exists for the given event type.
    MappingNotFound(String),

    /// Payload JSON is invalid or incompatible with the mapping.
    BuildFailed(String),

    /// Failed to write the record / guard / incident index to storage.
    WriteFailed(String),

    /// No upcaster chain from the event's version to the mapping's target version.
    UnsupportedVersion {
        event_type: String,
        from_version: u32,
        to_version: u32,
        reason: String,
    },
}

impl fmt::Display for GraphError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GraphError::MappingNotFound(event) => {
                write!(f, "no graph mapping found for event '{}'", event)
            }
            GraphError::BuildFailed(msg) => {
                write!(f, "failed to build graph projection: {}", msg)
            }
            GraphError::WriteFailed(msg) => {
                write!(f, "graph write failed: {}", msg)
            }
            GraphError::UnsupportedVersion {
                event_type,
                from_version,
                to_version,
                reason,
            } => write!(
                f,
                "cannot project event {:?} from v{} to mapping v{}: {}",
                event_type, from_version, to_version, reason
            ),
        }
    }
}

impl std::error::Error for GraphError {}
