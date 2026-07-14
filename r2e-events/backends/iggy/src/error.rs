use iggy::prelude::IggyError;
use r2e_events::EventBusError;

/// Map an `IggyError` to an `EventBusError`, classifying connection-related
/// errors by variant instead of substring matching.
pub fn map_iggy_error(err: IggyError) -> EventBusError {
    let is_conn = is_connection_error(&err);
    let msg = err.to_string();
    if is_conn {
        EventBusError::Connection(msg)
    } else {
        EventBusError::Other(msg)
    }
}

fn is_connection_error(err: &IggyError) -> bool {
    match err {
        IggyError::Disconnected
        | IggyError::CannotEstablishConnection
        | IggyError::NotConnected
        | IggyError::ClientShutdown
        | IggyError::StaleClient
        | IggyError::TcpError
        | IggyError::QuicError
        | IggyError::ConnectionClosed
        | IggyError::WebSocketError
        | IggyError::WebSocketConnectionError
        | IggyError::WebSocketCloseError
        | IggyError::WebSocketReceiveError
        | IggyError::WebSocketSendError
        | IggyError::CannotSendMessagesDueToClientDisconnection
        | IggyError::BackgroundSendError
        | IggyError::BackgroundSendTimeout
        | IggyError::BackgroundWorkerDisconnected
        | IggyError::ProducerClosed
        | IggyError::TaskTimeout => true,
        IggyError::CannotCloseWebSocketConnection(_)
        | IggyError::HttpError(_)
        | IggyError::IoError(_) => true,
        IggyError::ProducerSendFailed { cause, .. } => is_connection_error(cause),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_connection_and_transport_variants() {
        for error in [
            IggyError::ConnectionClosed,
            IggyError::WebSocketReceiveError,
            IggyError::TaskTimeout,
            IggyError::IoError("socket closed".to_string()),
        ] {
            assert!(matches!(
                map_iggy_error(error),
                EventBusError::Connection(_)
            ));
        }
    }

    #[test]
    fn configuration_errors_are_not_connection_errors() {
        assert!(matches!(
            map_iggy_error(IggyError::InvalidServerAddress),
            EventBusError::Other(_)
        ));
    }
}
