use r2e_events::EventBusError;

/// Map an `rdkafka::error::KafkaError` to an `EventBusError`.
pub fn map_kafka_error(err: rdkafka::error::KafkaError) -> EventBusError {
    let msg = err.to_string();
    if msg.contains("resolve")
        || msg.contains("connect")
        || msg.contains("broker")
        || msg.contains("timeout")
        || msg.contains("transport")
        || msg.contains("disconnected")
    {
        EventBusError::Connection(msg)
    } else {
        EventBusError::Other(msg)
    }
}
