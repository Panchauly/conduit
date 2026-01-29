use std::fmt;

/// Errors related to document projection and writing
#[derive(Debug)]
pub enum DocumentError {
    /// No mapping exists for the given event type
    MappingNotFound(String),

    /// Payload JSON is invalid or incompatible with mapping
    BuildFailed(String),

    /// Failed to write the document to storage
    WriteFailed(String),
}

impl fmt::Display for DocumentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DocumentError::MappingNotFound(event) => {
                write!(f, "no document mapping found for event '{}'", event)
            }
            DocumentError::BuildFailed(msg) => {
                write!(f, "failed to build document projection: {}", msg)
            }
            DocumentError::WriteFailed(msg) => {
                write!(f, "document write failed: {}", msg)
            }
        }
    }
}

impl std::error::Error for DocumentError {}
