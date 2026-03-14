use std::any::TypeId;
use std::collections::HashMap;

/// Registry mapping event `TypeId`s to Iggy topic names.
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

/// Sanitize a Rust type name into a valid Iggy topic name.
///
/// Replaces `::` with `.` and removes angle brackets / spaces.
pub fn sanitize_topic_name(name: &str) -> String {
    name.replace("::", ".")
        .replace('<', "_")
        .replace('>', "")
        .replace(' ', "")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_simple() {
        assert_eq!(
            sanitize_topic_name("my_crate::events::UserCreated"),
            "my_crate.events.UserCreated"
        );
    }

    #[test]
    fn sanitize_generic() {
        assert_eq!(
            sanitize_topic_name("my_crate::Wrapper<Inner>"),
            "my_crate.Wrapper_Inner"
        );
    }

    #[test]
    fn registry_lookup() {
        let mut reg = TopicRegistry::default();
        reg.register::<String>("custom-topic");

        assert_eq!(
            reg.resolve(TypeId::of::<String>(), "alloc::string::String"),
            "custom-topic"
        );
    }

    #[test]
    fn registry_fallback() {
        let reg = TopicRegistry::default();
        assert_eq!(
            reg.resolve(TypeId::of::<u32>(), "u32"),
            "u32"
        );
    }
}
