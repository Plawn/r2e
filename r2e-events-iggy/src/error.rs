use r2e_events::EventBusError;

/// Map an `IggyError` to an `EventBusError`.
pub fn map_iggy_error(err: iggy::prelude::IggyError) -> EventBusError {
    let msg = err.to_string();
    // Connection-related errors
    if msg.contains("connect")
        || msg.contains("transport")
        || msg.contains("disconnected")
        || msg.contains("timeout")
    {
        EventBusError::Connection(msg)
    } else {
        EventBusError::Other(msg)
    }
}
