#[derive(Debug)]
pub enum RuntimeError {
    NoAdaptersForRouting,
    AdapterConfigInvalid(String),
}
