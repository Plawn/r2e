//! Multi-round-trip requests (MRTR, protocol 2026-07-28): the sessionless
//! form of elicitation.
//!
//! A 2026-07-28 client has no channel a server can send a request on. A
//! member that needs input answers the call with an `InputRequiredResult`
//! instead: the questions (`inputRequests`) and an opaque `requestState`.
//! The client retries the **same** request with its answers
//! (`inputResponses`) and the echoed state, and the member runs again from
//! the top — this time [`McpClient::elicit`](crate::McpClient::elicit) finds
//! the answer under its key and returns it.
//!
//! Nothing is kept server-side: the answers of earlier rounds travel in the
//! state, signed with `mcp.request-state-key` (HMAC, **not** encrypted) and
//! bound to the caller, the method, the member and its arguments, so any
//! replica holding the key can serve any round.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

use rmcp::model::{
    ElicitRequest, ElicitRequestParams, ErrorCode, ErrorData, InputRequest, InputRequests,
    InputRequiredResult, InputResponses, RequestStateCodec, SealOptions,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Default `mcp.mrtr-max-rounds`.
pub(crate) const DEFAULT_MAX_ROUNDS: u32 = 10;

/// The endpoint-wide MRTR settings; present only when
/// `mcp.request-state-key` is configured.
pub(crate) struct Mrtr {
    codec: RequestStateCodec,
    max_rounds: u32,
    /// How long the client has to come back with the answers
    /// (`mcp.elicitation-timeout-secs`).
    ttl: Duration,
}

/// What the sealed `requestState` carries between two rounds.
#[derive(Serialize, Deserialize)]
struct State {
    /// Rounds already answered with `input_required`.
    r: u32,
    /// The keys asked in the previous round — the only ones whose answers
    /// are accepted from this retry.
    q: Vec<String>,
    /// Answers gathered in earlier rounds, by key.
    a: BTreeMap<String, Value>,
}

/// One round of one call: the answers known so far and the questions the
/// member asked that have none yet.
pub(crate) struct Round {
    round: u32,
    answers: BTreeMap<String, Value>,
    pending: Mutex<InputRequests>,
}

impl Round {
    /// The client's answer (an `ElicitResult` as JSON) to the question
    /// asked under `key`, when a previous round got one.
    pub(crate) fn answer(&self, key: &str) -> Option<&Value> {
        self.answers.get(key)
    }

    /// Record a question for the client. The first one asked under a key
    /// wins: a key identifies one question within a call.
    pub(crate) fn ask(&self, key: &str, params: ElicitRequestParams) {
        self.pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entry(key.to_owned())
            .or_insert_with(|| InputRequest::Elicitation(ElicitRequest::new(params)));
    }
}

/// A [`Round`] and what its state is bound to.
pub(crate) struct MrtrCall {
    pub(crate) round: std::sync::Arc<Round>,
    binding: Vec<u8>,
}

impl Mrtr {
    /// `key` is `mcp.request-state-key`; fails when shorter than rmcp's
    /// 32-byte minimum.
    pub(crate) fn new(key: &str, max_rounds: u32, ttl: Duration) -> Result<Self, String> {
        let codec = RequestStateCodec::try_new(key.as_bytes().to_vec())
            .map_err(|err| format!("mcp.request-state-key: {err}"))?;
        Ok(Mrtr {
            codec,
            max_rounds,
            ttl,
        })
    }

    /// Open the round a request belongs to: the first one when it carries
    /// no state, otherwise the one its (verified) state describes plus the
    /// answers it brings.
    pub(crate) fn begin(
        &self,
        binding: Vec<u8>,
        responses: Option<InputResponses>,
        state: Option<&str>,
    ) -> Result<MrtrCall, ErrorData> {
        let (round, answers) = match state {
            // Answers nobody asked for are not answers.
            None => (0, BTreeMap::new()),
            Some(sealed) => {
                // One message for every failure (tampered, expired, sealed
                // for another call): the cause is not the client's business.
                let state: State = self.codec.open_json_with(sealed, &binding).map_err(|_| {
                    ErrorData::new(
                        ErrorCode::INVALID_PARAMS,
                        "invalid or expired requestState: retry the request without it",
                        None,
                    )
                })?;
                let mut answers = state.a;
                let mut responses = responses.unwrap_or_default();
                for key in state.q {
                    if let Some(answer) = responses.remove(&key) {
                        answers.insert(key, answer);
                    }
                }
                (state.r, answers)
            }
        };
        Ok(MrtrCall {
            round: std::sync::Arc::new(Round {
                round,
                answers,
                pending: Mutex::new(InputRequests::new()),
            }),
            binding,
        })
    }

    /// Close the round once the member returned: `Some` when it asked a
    /// question that has no answer yet — the result to send instead of the
    /// member's own.
    pub(crate) fn finish(&self, call: MrtrCall) -> Result<Option<InputRequiredResult>, ErrorData> {
        let pending = std::mem::take(
            &mut *call
                .round
                .pending
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        );
        if pending.is_empty() {
            return Ok(None);
        }
        let round = call.round.round + 1;
        if round > self.max_rounds {
            return Err(ErrorData::new(
                ErrorCode::INVALID_REQUEST,
                format!(
                    "the request still needs input after {} rounds (mcp.mrtr-max-rounds)",
                    self.max_rounds
                ),
                None,
            ));
        }
        let state = State {
            r: round,
            q: pending.keys().cloned().collect(),
            a: call.round.answers.clone(),
        };
        let sealed = self
            .codec
            .seal_json_with(
                &state,
                &SealOptions::new()
                    .associated_data(&call.binding)
                    .ttl(self.ttl),
            )
            .map_err(|err| ErrorData::new(ErrorCode::INTERNAL_ERROR, err.to_string(), None))?;
        Ok(Some(InputRequiredResult::new(Some(pending), Some(sealed))))
    }
}

/// What a state is valid for: this caller, this method, this member, these
/// arguments. An answer given for `delete(id = 1)` cannot be replayed on
/// `delete(id = 2)`, nor by another principal.
pub(crate) fn binding(
    method: &str,
    target: &str,
    subject: Option<&str>,
    arguments: Option<&serde_json::Map<String, Value>>,
) -> Vec<u8> {
    let mut out = String::new();
    for part in [method, target, subject.unwrap_or("")] {
        // Length-prefixed: no field can bleed into the next one.
        out.push_str(&part.len().to_string());
        out.push(':');
        out.push_str(part);
    }
    if let Some(arguments) = arguments {
        canonical_object(arguments, &mut out);
    }
    out.into_bytes()
}

/// JSON with object keys sorted, so the binding does not depend on the order
/// a client serializes its arguments in from one round to the next.
fn canonical(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => canonical_object(map, out),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                canonical(item, out);
            }
            out.push(']');
        }
        scalar => out.push_str(&scalar.to_string()),
    }
}

fn canonical_object(map: &serde_json::Map<String, Value>, out: &mut String) {
    let mut entries: Vec<_> = map.iter().collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));
    out.push('{');
    for (i, (key, value)) in entries.into_iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&Value::String(key.clone()).to_string());
        out.push(':');
        canonical(value, out);
    }
    out.push('}');
}
