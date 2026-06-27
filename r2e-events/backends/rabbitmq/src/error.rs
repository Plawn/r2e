use r2e_events::EventBusError;

/// Map a `lapin::Error` to an `EventBusError`.
pub fn map_lapin_error(err: lapin::Error) -> EventBusError {
    use lapin::ErrorKind;
    match err.kind() {
        ErrorKind::IOError(_)
        | ErrorKind::ProtocolError(_)
        | ErrorKind::InvalidChannelState(..)
        | ErrorKind::InvalidConnectionState(_) => {
            EventBusError::Connection(err.to_string())
        }
        ErrorKind::SerialisationError(_) => {
            EventBusError::Serialization(err.to_string())
        }
        _ => EventBusError::Other(err.to_string()),
    }
}
