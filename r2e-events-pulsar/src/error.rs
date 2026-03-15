use r2e_events::EventBusError;

/// Map a `pulsar::Error` to an `EventBusError`.
pub fn map_pulsar_error(err: pulsar::Error) -> EventBusError {
    let msg = err.to_string();
    // Connection-related errors
    if msg.contains("connect")
        || msg.contains("Connection")
        || msg.contains("disconnected")
        || msg.contains("timeout")
        || msg.contains("Timeout")
        || msg.contains("ServiceDiscovery")
    {
        EventBusError::Connection(msg)
    } else {
        EventBusError::Other(msg)
    }
}
