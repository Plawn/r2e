use r2e_events::EventBusError;
use rdkafka::error::KafkaError;
use rdkafka::types::RDKafkaErrorCode;

fn is_connection_error_code(code: RDKafkaErrorCode) -> bool {
    matches!(
        code,
        RDKafkaErrorCode::BrokerDestroy
            | RDKafkaErrorCode::Fail
            | RDKafkaErrorCode::BrokerTransportFailure
            | RDKafkaErrorCode::Resolve
            | RDKafkaErrorCode::AllBrokersDown
            | RDKafkaErrorCode::OperationTimedOut
            | RDKafkaErrorCode::MessageTimedOut
            | RDKafkaErrorCode::TimedOutQueue
    )
}

/// Map an `rdkafka::error::KafkaError` to an `EventBusError`.
pub fn map_kafka_error(err: KafkaError) -> EventBusError {
    // `rdkafka_error_code` covers every code-bearing variant except
    // `AdminOp`, whose code is exposed directly by the enum.
    let code = match &err {
        KafkaError::AdminOp(code) => Some(*code),
        _ => err.rdkafka_error_code(),
    };
    let is_conn = code.is_some_and(is_connection_error_code);
    let msg = err.to_string();
    if is_conn {
        EventBusError::Connection(msg)
    } else {
        EventBusError::Other(msg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_codes_across_kafka_error_variants() {
        assert!(matches!(
            map_kafka_error(KafkaError::AdminOp(RDKafkaErrorCode::OperationTimedOut)),
            EventBusError::Connection(_)
        ));
        assert!(matches!(
            map_kafka_error(KafkaError::StoreOffset(
                RDKafkaErrorCode::BrokerTransportFailure
            )),
            EventBusError::Connection(_)
        ));
    }

    #[test]
    fn subscription_text_is_not_assumed_to_be_a_connection_error() {
        assert!(matches!(
            map_kafka_error(KafkaError::Subscription("invalid topic".to_string())),
            EventBusError::Other(_)
        ));
    }
}
