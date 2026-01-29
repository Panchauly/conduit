use std::collections::HashMap;

use crate::adapter::StorageAdapter;
use crate::adapter::document::file::FileDocumentAdapter;
use crate::adapter::document::mapping::DocumentMapping;
use crate::adapter::document::runtime::DocumentRuntimeBuilder;
use crate::adapter::sql::mapping::SqlMapping;
use crate::adapter::sql::runtime::SqlRuntimeBuilder;
use crate::adapter::sql::sqlite::SqliteAdapter;

pub fn build_adapters(
    sql_mappings: HashMap<String, SqlMapping>,
    doc_mappings: HashMap<String, DocumentMapping>,
) -> Vec<Box<dyn StorageAdapter>> {
    let mut adapters: Vec<Box<dyn StorageAdapter>> = Vec::new();

    if !sql_mappings.is_empty() {
        let builder = SqlRuntimeBuilder::new(sql_mappings);

        adapters.push(Box::new(SqliteAdapter::new(
            "sqlite".to_string(),
            "./data/app.db".to_string(),
            10,
            builder,
        )));
    }

    if !doc_mappings.is_empty() {
        let builder = DocumentRuntimeBuilder::new(doc_mappings);

        adapters.push(Box::new(FileDocumentAdapter::new(
            "file".to_string(),
            "./out/documents".into(),
            20,
            builder,
        )));
    }

    adapters
}
