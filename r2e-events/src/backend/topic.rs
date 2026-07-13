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

    /// Register a topic name by raw `TypeId` (for runtime registration).
    pub fn register_by_type_id(&mut self, type_id: TypeId, topic: impl Into<String>) {
        self.map.insert(type_id, topic.into());
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

/// Suffix appended to an event topic to form its request-reply request topic.
pub const REQUEST_TOPIC_SUFFIX: &str = ".requests";

/// The request topic for an event topic: `<event_topic>.requests`.
///
/// Requesters publish to this shared topic; the responder (one per request
/// type per consumer group) consumes it and replies to the per-request
/// [`reply_topic`].
pub fn request_topic(event_topic: &str) -> String {
    format!("{event_topic}{REQUEST_TOPIC_SUFFIX}")
}

/// The per-bus-instance reply topic: `<topic_prefix>.replies.<instance-id-hex>`.
///
/// Each bus instance consumes its own reply topic; responders publish replies
/// here (named by the request's `reply-to` header) and the instance's reply
/// consumer correlates them back to the waiting requester.
///
/// `instance_id` must be a per-bus-instance nonce (mint one with
/// [`instance_id`]), NOT the process id: two bus instances sharing a config in
/// one process would otherwise collide on the same reply topic and steal each
/// other's replies.
pub fn reply_topic(topic_prefix: &str, instance_id: u64) -> String {
    format!("{topic_prefix}.replies.{instance_id:016x}")
}

/// Mint a fresh random 64-bit nonce identifying one bus instance.
///
/// Drawn from the OS CSPRNG (via `uuid`). Each call returns a new value; a
/// backend calls this once per bus instance and passes the result to
/// [`reply_topic`] so its reply topic is unique even when several bus instances
/// share a config within a single process.
pub fn instance_id() -> u64 {
    let (hi, lo) = uuid::Uuid::new_v4().as_u64_pair();
    hi ^ lo
}
