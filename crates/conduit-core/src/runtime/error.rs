use core::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeError {
    NoAdaptersForRouting,
    AdapterConfigInvalid(String),
    /// Global routing table source (file at `ROUTING_CONFIG`/`routing.json`) could not be read.
    RoutingConfigUnreadable(String),
    /// Global routing table source could not be parsed as JSON.
    RoutingConfigInvalid(String),
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RuntimeError::NoAdaptersForRouting => write!(f, "no adapters available for routing"),
            RuntimeError::AdapterConfigInvalid(msg) => write!(f, "invalid adapter config: {}", msg),
            RuntimeError::RoutingConfigUnreadable(msg) => {
                write!(f, "failed to read routing config: {}", msg)
            }
            RuntimeError::RoutingConfigInvalid(msg) => {
                write!(f, "invalid routing config: {}", msg)
            }
        }
    }
}

impl std::error::Error for RuntimeError {}
