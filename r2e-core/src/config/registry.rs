use std::sync::Mutex;

use super::typed::{ConfigProperties, PropertyMeta};

static CONFIG_REGISTRY: Mutex<Vec<RegisteredSection>> = Mutex::new(Vec::new());

/// A registered configuration section with its metadata.
#[derive(Debug, Clone)]
pub struct RegisteredSection {
    pub prefix: String,
    pub properties: Vec<PropertyMeta>,
}

/// Register a config section's metadata in the global registry.
pub fn register_section<C: ConfigProperties>() {
    let section = RegisteredSection {
        prefix: C::prefix().to_string(),
        properties: C::properties_metadata(),
    };
    CONFIG_REGISTRY.lock().unwrap().push(section);
}

/// Get all registered config sections.
pub fn registered_sections() -> Vec<RegisteredSection> {
    CONFIG_REGISTRY.lock().unwrap().clone()
}
