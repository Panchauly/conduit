use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
pub struct DocumentMapping {
    pub event: String,
    pub collection: String,

    /// JSON-like structure where leaf values are payload/metadata paths
    pub document: Value,
}
