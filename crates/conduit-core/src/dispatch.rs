use crate::adapter::{AdapterResult, StorageAdapter};
use crate::event::Event;
use crate::routing::route;

pub fn dispatch(event: &Event, adapters: &mut [Box<dyn StorageAdapter>]) -> Vec<AdapterResult> {
    let targets = route(event);

    let mut results = Vec::new();

    for adapter in adapters.iter() {
        if targets.contains(&adapter.kind()) {
            let result = adapter.handle(event);
            results.push(result);
        }
    }

    results
}
