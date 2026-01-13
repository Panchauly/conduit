use crate::adapter::{AdapterError, StorageAdapter};
use crate::event::Event;
use crate::routing::{StorageKind, route};

pub fn dispatch(
    event: &Event,
    adapters: &[Box<dyn StorageAdapter>],
) -> Vec<(StorageKind, Result<(), AdapterError>)> {
    let targets = route(event);
    let mut results = Vec::new();

    for adapter in adapters {
        if targets.contains(&adapter.kind()) {
            let result = adapter.handle(event);
            results.push((adapter.kind(), result));
        }
    }

    results
}
