//! Phase 25.2: the adapter factory registry — the mechanism behind
//! `AdapterConfig::Custom`. An embedding application registers a factory for
//! an unrecognized `type:` string before calling [`crate::runtime::build_adapters_from_config`];
//! from that point on `type: mysql` (or any other name) in the YAML config
//! resolves through here exactly like a built-in adapter type resolves
//! through a fixed `match` arm.
//!
//! **No dynamic loading.** This is Rust's ordinary compile-time extension
//! model — implement [`crate::adapter::StorageAdapter`] (directly, or via one
//! of the four public backend seams), depend on `conduit-core`, call
//! [`register_adapter_factory`] at startup. `dlopen`/FFI plugin loading is a
//! deliberate non-goal (see the phase doc).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use once_cell::sync::Lazy;

use crate::adapter::StorageAdapter;
use crate::runtime::config::ConfigError;

/// A registered adapter factory: the adapter's entire raw YAML list entry in
/// (`type`, `id`, `priority`, `config`, `capabilities`, `depends_on` — see
/// [`crate::runtime::config::CustomAdapterConfig`]'s doc comment for why the
/// whole entry rather than just `config:`), a fully built adapter out. Must
/// be `Send + Sync` to live in the global registry —
/// a tightening of the phase doc's literal sketch (`impl Fn(...) + 'static`),
/// required because the registry is a process-wide `static`; see phase-25's
/// "As built" notes.
pub type AdapterFactory =
    Arc<dyn Fn(serde_yaml::Value) -> Result<Box<dyn StorageAdapter>, ConfigError> + Send + Sync>;

static REGISTRY: Lazy<Mutex<HashMap<String, AdapterFactory>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// Register a factory for adapter `type: <type_name>`. Call this before
/// [`crate::runtime::build_adapters_from_config`] runs — typically at process
/// startup, in the embedding application's `main` (the "embedded crate"
/// deployment mode, `architecture.md` §1.3). Registering the same
/// `type_name` twice replaces the earlier factory.
pub fn register_adapter_factory(
    type_name: &str,
    factory: impl Fn(serde_yaml::Value) -> Result<Box<dyn StorageAdapter>, ConfigError>
    + Send
    + Sync
    + 'static,
) {
    if let Ok(mut registry) = REGISTRY.lock() {
        registry.insert(type_name.to_string(), Arc::new(factory));
    }
}

/// Look up and invoke the factory for `type_name`. `None` means nothing is
/// registered under that name; `Some(Err(_))` means the factory itself
/// rejected the config.
pub(crate) fn build(
    type_name: &str,
    config: serde_yaml::Value,
) -> Option<Result<Box<dyn StorageAdapter>, ConfigError>> {
    let factory = REGISTRY.lock().ok()?.get(type_name).cloned()?;
    Some(factory(config))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::{AdapterResult, StorageAdapter};
    use crate::event::Event;
    use crate::routing::StorageKind;

    struct Noop;
    impl StorageAdapter for Noop {
        fn kind(&self) -> StorageKind {
            StorageKind::KeyValue
        }
        fn id(&self) -> &str {
            "noop"
        }
        fn priority(&self) -> u32 {
            0
        }
        fn handle(&self, _event: &Event) -> AdapterResult {
            AdapterResult::created(self.id().to_string(), self.kind())
        }
    }

    #[test]
    fn unregistered_type_resolves_to_none() {
        assert!(build("definitely-not-registered-xyz", serde_yaml::Value::Null).is_none());
    }

    #[test]
    fn registered_factory_is_invoked_with_its_config_block() {
        register_adapter_factory("test-noop-registry", |cfg| {
            assert_eq!(cfg.as_str(), Some("hello"));
            Ok(Box::new(Noop) as Box<dyn StorageAdapter>)
        });
        let result = build(
            "test-noop-registry",
            serde_yaml::Value::String("hello".into()),
        );
        assert!(matches!(result, Some(Ok(_))));
    }
}
