pub mod document;
pub mod sql;

use crate::event::Event;
use crate::routing::StorageKind;

#[derive(Debug)]
pub enum AdapterError {
    WriteFailed(String),
}

pub trait StorageAdapter {
    fn kind(&self) -> StorageKind;
    fn handle(&self, event: &Event) -> Result<(), AdapterError>;
}
