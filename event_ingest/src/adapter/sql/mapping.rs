use std::collections::HashMap;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct SqlMapping {
    pub event: String,
    pub table: String,

    pub primary_key: String,

    pub columns: HashMap<String, String>,

    #[serde(default)]
    pub foreign_keys: HashMap<String, String>,
}
