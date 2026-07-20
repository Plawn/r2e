use r2e_events::EventBusError;

use lapin::Confirmation;

/// Map a `lapin::Error` to an `EventBusError`.
pub fn map_lapin_error(err: lapin::Error) -> EventBusError {
    use lapin::ErrorKind;
    match err.kind() {
        ErrorKind::IOError(_)
        | ErrorKind::ProtocolError(_)
        | ErrorKind::InvalidChannelState(..)
        | ErrorKind::InvalidConnectionState(_) => EventBusError::Connection(err.to_string()),
        ErrorKind::SerialisationError(_) => EventBusError::Serialization(err.to_string()),
        _ => EventBusError::Other(err.to_string()),
    }
}

/// Require a positive RabbitMQ publisher confirmation.
///
/// `PublisherConfirm::await` only reports protocol/connection failures through
/// its outer `Result`; broker nacks and channels without confirm mode are
/// successful `Result`s carrying a non-ack [`Confirmation`].
pub(crate) fn require_publisher_ack(confirmation: Confirmation) -> Result<(), EventBusError> {
    match confirmation {
        Confirmation::Ack(None) => Ok(()),
        Confirmation::Ack(Some(_)) => Err(EventBusError::Other(
            "RabbitMQ returned the published message as unroutable".to_string(),
        )),
        Confirmation::Nack(_) => Err(EventBusError::Other(
            "RabbitMQ negatively acknowledged the published message".to_string(),
        )),
        Confirmation::NotRequested => Err(EventBusError::Other(
            "RabbitMQ publisher confirms are not enabled on this channel".to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_clean_ack_is_successful() {
        assert!(require_publisher_ack(Confirmation::Ack(None)).is_ok());
        assert!(require_publisher_ack(Confirmation::Nack(None)).is_err());
        assert!(require_publisher_ack(Confirmation::NotRequested).is_err());
    }
}
