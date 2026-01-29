use std::fmt;

/// SQL-side adapter / builder errors
#[derive(Debug)]
pub enum SqlError {
    MappingNotFound(String),
    BuildFailed(String),
    ExecutionFailed(String),
}

impl fmt::Display for SqlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SqlError::MappingNotFound(event) => {
                write!(f, "no SQL mapping found for event '{}'", event)
            }
            SqlError::BuildFailed(msg) => {
                write!(f, "failed to build SQL projection: {}", msg)
            }
            SqlError::ExecutionFailed(msg) => {
                write!(f, "SQL execution failed: {}", msg)
            }
        }
    }
}

impl std::error::Error for SqlError {}
