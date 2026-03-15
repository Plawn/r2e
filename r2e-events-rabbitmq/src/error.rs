use r2e_events::EventBusError;

/// Map a `lapin::Error` to an `EventBusError`.
pub fn map_lapin_error(err: lapin::Error) -> EventBusError {
    match &err {
        lapin::Error::IOError(_)
        | lapin::Error::ProtocolError(_)
        | lapin::Error::InvalidChannelState(_)
        | lapin::Error::InvalidConnectionState(_) => {
            EventBusError::Connection(err.to_string())
        }
        lapin::Error::SerialisationError(_) => {
            EventBusError::Serialization(err.to_string())
        }
        _ => EventBusError::Other(err.to_string()),
    }
}
