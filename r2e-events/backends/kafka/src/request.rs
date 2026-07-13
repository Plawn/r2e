// NOTE: The `rdkafka` (librdkafka) consumer is tokio-bound; any tokio APIs
// that originate from the rdkafka SDK remain on direct tokio and are a
// documented exception to the r2e_core::rt facade.
//! Request-reply transport for `KafkaEventBus` (ReplyingKafkaTemplate pattern).
//!
//! - Requesters publish to a shared request topic (`<event-topic>.requests`)
//!   and consume replies on a per-instance, instance-private reply topic
//!   (`<group-id>.replies.<instance-id-hex>`), correlated by a `u128` id.
//! - Responders (`respond`) consume the shared request topic with the
//!   configured consumer group — the broker load-balances requests across
//!   instances — and publish each reply to the `reply-to` topic named in the
//!   request headers.
//!
//! An absent responder manifests to the requester as a
//! [`RequestTimeout`](r2e_events::EventBusError::RequestTimeout): no instance
//! is subscribed to the request topic, so nothing ever replies.

use std::any::TypeId;
use std::sync::Arc;
use std::time::Duration;

use rdkafka::consumer::{CommitMode, Consumer, StreamConsumer};
use rdkafka::Message;
use tokio_util::sync::CancellationToken;

use r2e_events::backend::{
    decode_metadata, decode_reply_headers, encode_reply_headers, reconnect_loop, ReplyHeaders,
};
use r2e_events::EventMetadata;

use crate::bus::{kafka_header_pairs, produce_with_headers};
use crate::inner::KafkaInner;

/// Build the unique, per-instance consumer group for the reply consumer.
///
/// Distinct from the configured group so each instance sees only its own
/// reply topic — reply delivery is point-to-point back to the exact requester,
/// never load-balanced. Embeds the same `instance_id` as the instance's reply
/// topic, so a process running two bus instances gets disjoint reply groups too.
pub(crate) fn reply_consumer_group(config_group_id: &str, instance_id: u64) -> String {
    format!("{config_group_id}.reply-consumer.{instance_id:016x}")
}

/// Background loop consuming this process's private reply topic, routing each
/// reply to the waiting requester by correlation id. Reconnects with backoff
/// (mirrors `run_consumer`) until cancelled.
pub(crate) async fn run_reply_consumer(
    inner: Arc<KafkaInner>,
    topic_name: String,
    cancel: CancellationToken,
) {
    let label = format!("Kafka reply consumer [{topic_name}]");
    reconnect_loop(
        inner.config.reconnect,
        inner.config.reconnect_max_backoff,
        &cancel,
        &label,
        || run_reply_consumer_inner(&inner, &topic_name, &cancel),
    )
    .await;
}

async fn run_reply_consumer_inner(
    inner: &Arc<KafkaInner>,
    topic_name: &str,
    cancel: &CancellationToken,
) {
    let mut cfg = inner.config.to_consumer_client_config();
    // Instance-private group; read from the beginning so a reply produced
    // during the join window is not lost (the topic is process-private, so
    // "earliest" only ever yields our own replies).
    cfg.set("group.id", reply_consumer_group(&inner.config.group_id, inner.instance_id));
    cfg.set("auto.offset.reset", "earliest");
    cfg.set("enable.auto.commit", "true");
    cfg.set("enable.auto.offset.store", "true");

    let consumer: StreamConsumer = match cfg.create() {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(topic = %topic_name, "failed to create Kafka reply consumer: {e}");
            return;
        }
    };

    if let Err(e) = consumer.subscribe(&[topic_name]) {
        tracing::error!(topic = %topic_name, "failed to subscribe to Kafka reply topic: {e}");
        return;
    }

    tracing::info!(topic = %topic_name, "Kafka reply consumer started");

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                tracing::info!(topic = %topic_name, "Kafka reply consumer cancelled");
                break;
            }
            msg = consumer.recv() => {
                match msg {
                    Ok(m) => {
                        let pairs = kafka_header_pairs(&m);
                        let Some(reply) = decode_reply_headers(pairs.iter().map(|(k, v)| (k, v))) else {
                            tracing::warn!(topic = %topic_name, "reply message missing request id");
                            continue;
                        };
                        // Single-source the Remote-vs-Ok decision via the shared
                        // helper: a reply-error header becomes `Remote`, otherwise
                        // the payload bytes are delivered to the waiting requester.
                        inner
                            .pending
                            .complete_reply(&reply, m.payload().unwrap_or_default().to_vec());
                    }
                    Err(e) => {
                        tracing::warn!(topic = %topic_name, "Kafka reply consumer error: {e}");
                        r2e_core::rt::sleep(Duration::from_secs(1)).await;
                    }
                }
            }
        }
    }
}

/// Background loop consuming the shared request topic for one request type,
/// invoking the registered responder and publishing each reply to the
/// requester's reply topic. Reconnects with backoff until cancelled.
pub(crate) async fn run_responder_consumer(
    inner: Arc<KafkaInner>,
    type_id: TypeId,
    topic_name: String,
    cancel: CancellationToken,
) {
    let label = format!("Kafka responder consumer [{topic_name}]");
    reconnect_loop(
        inner.config.reconnect,
        inner.config.reconnect_max_backoff,
        &cancel,
        &label,
        || run_responder_consumer_inner(&inner, type_id, &topic_name, &cancel),
    )
    .await;
}

async fn run_responder_consumer_inner(
    inner: &Arc<KafkaInner>,
    type_id: TypeId,
    topic_name: &str,
    cancel: &CancellationToken,
) {
    // Consume with the configured group so requests are load-balanced across
    // instances (queue semantics). Offsets are committed per message only after
    // the reply is produced — reply-then-commit keeps at-least-once delivery.
    let consumer: StreamConsumer = match inner.config.to_consumer_client_config().create() {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(topic = %topic_name, "failed to create Kafka responder consumer: {e}");
            return;
        }
    };

    if let Err(e) = consumer.subscribe(&[topic_name]) {
        tracing::error!(topic = %topic_name, "failed to subscribe to Kafka request topic: {e}");
        return;
    }

    tracing::info!(topic = %topic_name, "Kafka responder started");

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                tracing::info!(topic = %topic_name, "Kafka responder cancelled");
                break;
            }
            msg = consumer.recv() => {
                match msg {
                    Ok(m) => {
                        let payload = match m.payload() {
                            Some(p) => p.to_vec(),
                            None => {
                                tracing::warn!(topic = %topic_name, "received request with no payload");
                                let _ = consumer.commit_message(&m, CommitMode::Async);
                                continue;
                            }
                        };

                        let pairs = kafka_header_pairs(&m);
                        let reply = decode_reply_headers(pairs.iter().map(|(k, v)| (k, v)));
                        let metadata = decode_metadata(pairs.into_iter());

                        // Reply-then-commit for at-least-once: process fully,
                        // publish the reply, then advance the offset — but ONLY
                        // if the reply was actually produced. A failed reply
                        // produce leaves the offset uncommitted so the broker
                        // redelivers the request and the requester can be served.
                        // (A responder error whose error reply produced fine still
                        // commits — only a failed reply PRODUCE skips the commit.)
                        if handle_request(inner, type_id, &payload, metadata, reply).await {
                            if let Err(e) = consumer.commit_message(&m, CommitMode::Async) {
                                // ERR__NO_OFFSET and transient commit errors are
                                // benign — the request is simply redelivered.
                                tracing::debug!(topic = %topic_name, "responder commit skipped: {e}");
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(topic = %topic_name, "Kafka responder error: {e}");
                        r2e_core::rt::sleep(Duration::from_secs(1)).await;
                    }
                }
            }
        }
    }
}

/// Invoke the responder for `type_id` and publish its reply to `reply_to`.
///
/// Returns whether the request's offset may be committed:
/// - `true` — the reply was produced (or the request was malformed and can only
///   be dropped): safe to advance the offset.
/// - `false` — producing the reply FAILED: leave the offset uncommitted so the
///   broker redelivers the request.
async fn handle_request(
    inner: &Arc<KafkaInner>,
    type_id: TypeId,
    payload: &[u8],
    metadata: EventMetadata,
    reply: Option<ReplyHeaders>,
) -> bool {
    let Some(reply) = reply else {
        tracing::warn!("request missing request id; dropping");
        return true;
    };
    let Some(reply_to) = reply.reply_to else {
        tracing::warn!(request_id = reply.request_id, "request missing reply-to; dropping");
        return true;
    };

    // Single-sourced outcome mapping (incl. the no-responder error reply).
    let (reply_payload, reply_error) = inner.state.build_reply(type_id, payload, metadata).await;

    let headers = encode_reply_headers(reply.request_id, None, reply_error.as_deref());
    match produce_with_headers(&inner.producer, &reply_to, &reply_payload, None, headers).await {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!(
                topic = %reply_to,
                request_id = reply.request_id,
                "failed to publish reply; not committing request offset: {e}"
            );
            false
        }
    }
}
