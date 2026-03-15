use std::any::TypeId;
use std::collections::HashMap;

/// Registry mapping event `TypeId`s to topic names.
#[derive(Debug, Default)]
pub struct TopicRegistry {
    map: HashMap<TypeId, String>,
}

impl TopicRegistry {
    /// Register an explicit topic name for event type `E`.
    pub fn register<E: 'static>(&mut self, topic: impl Into<String>) {
        self.map.insert(TypeId::of::<E>(), topic.into());
    }

    /// Resolve the topic name for a given `TypeId`.
    ///
    /// Returns the registered name, or falls back to a sanitized `type_name`.
    pub fn resolve(&self, type_id: TypeId, type_name: &str) -> String {
        self.map
            .get(&type_id)
            .cloned()
            .unwrap_or_else(|| sanitize_topic_name(type_name))
    }
}

/// Sanitize a Rust type name into a valid topic name.
///
/// Replaces `::` with `.` and removes angle brackets / spaces.
pub fn sanitize_topic_name(name: &str) -> String {
    name.replace("::", ".")
        .replace('<', "_")
        .replace('>', "")
        .replace(' ', "")
}
