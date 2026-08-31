use crate::config::{ConfigError, ConfigValue, FromConfigValue};
use crate::http::response::{IntoHttpResponse, IntoResponse, Response};
use crate::http::{Json, StatusCode};
use std::sync::atomic::{AtomicU8, Ordering};

/// Config key selecting the body format of a `#[derive(Params)]` 400.
pub const PARAMS_REJECTION_FORMAT_KEY: &str = "server.params-rejection-format";

/// Body format of the `400 Bad Request` a `#[derive(Params)]` extraction
/// failure produces.
///
/// This is an **app-level** decision — one setting for the whole application,
/// resolved once at app construction from
/// [`PARAMS_REJECTION_FORMAT_KEY`](PARAMS_REJECTION_FORMAT_KEY) — never a
/// per-struct or per-route one: a client parses one error shape from an API,
/// not one per DTO.
///
/// ```yaml
/// server:
///   params-rejection-format: plain-text   # default: json
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ParamsRejectionFormat {
    /// `{"error": "<message>"}` with `content-type: application/json` — the
    /// R2E default, matching every other framework-produced error body.
    #[default]
    Json,
    /// The bare message as `text/plain` — byte-for-byte what a raw
    /// `Query<T>` rejection returns, so a migration from `Query<T>` to
    /// `#[derive(Params)]` does not change what existing clients read.
    PlainText,
}

impl ParamsRejectionFormat {
    fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::PlainText,
            _ => Self::Json,
        }
    }

    fn as_u8(self) -> u8 {
        match self {
            Self::Json => 0,
            Self::PlainText => 1,
        }
    }
}

impl FromConfigValue for ParamsRejectionFormat {
    fn from_config_value(value: &ConfigValue, key: &str) -> Result<Self, ConfigError> {
        let raw = String::from_config_value(value, key)?;
        match raw.trim().to_ascii_lowercase().as_str() {
            "json" => Ok(Self::Json),
            "plain-text" | "plain_text" | "plaintext" | "text" => Ok(Self::PlainText),
            _ => Err(ConfigError::TypeMismatch {
                key: key.to_string(),
                expected: "one of `json`, `plain-text`",
            }),
        }
    }
}

/// Process-global slot for the resolved format.
///
/// Written once per `build_state()` (including every `r2e dev` hot-patch
/// cycle, so a config edit takes effect) and only ever read on the rejection
/// path, where a relaxed atomic load is free. It is a global rather than a
/// bean because `#[derive(Params)]` extracts against a **state-generic** `S`
/// with no `BeanLookup` bound — the derive must work for any state, including
/// the core-only one.
static FORMAT: AtomicU8 = AtomicU8::new(0);

/// Install the app-level rejection format. Called by `build_state()`.
pub fn set_params_rejection_format(format: ParamsRejectionFormat) {
    FORMAT.store(format.as_u8(), Ordering::Relaxed);
}

/// The rejection format in force for this process.
pub fn params_rejection_format() -> ParamsRejectionFormat {
    ParamsRejectionFormat::from_u8(FORMAT.load(Ordering::Relaxed))
}

/// Error type for parameter extraction failures in `#[derive(Params)]`.
#[derive(Debug)]
pub struct ParamError {
    pub message: String,
}

impl std::fmt::Display for ParamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl IntoHttpResponse for ParamError {
    fn into_http_response(self) -> Response {
        match params_rejection_format() {
            ParamsRejectionFormat::Json => {
                let body = serde_json::json!({ "error": self.message });
                (StatusCode::BAD_REQUEST, Json(body)).into_response()
            }
            ParamsRejectionFormat::PlainText => {
                (StatusCode::BAD_REQUEST, self.message).into_response()
            }
        }
    }
}

crate::http::impl_into_response!(ParamError);

impl From<ParamError> for Response {
    fn from(err: ParamError) -> Self {
        err.into_response()
    }
}

/// Parse a query string into key-value pairs.
pub fn parse_query_string(query: Option<&str>) -> Vec<(String, String)> {
    match query {
        Some(q) => form_urlencoded::parse(q.as_bytes())
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect(),
        None => Vec::new(),
    }
}

// ── ParamsMetadata: OpenAPI parameter metadata for #[derive(Params)] ──

use crate::di::meta::ParamInfo;

/// Trait for types that expose parameter metadata for OpenAPI spec generation.
/// Auto-implemented by `#[derive(Params)]`.
pub trait ParamsMetadata {
    fn param_infos() -> Vec<ParamInfo>;
}

// ── Autoref specialization support ──
//
// Allows generated code to call `param_infos()` on any handler parameter type:
// - Types implementing ParamsMetadata → returns real metadata (inherent method)
// - Other types → returns empty vec (fallback via autoref to __NoParamsMeta)

#[doc(hidden)]
pub struct __ParamMetaProbe<T>(pub core::marker::PhantomData<T>);

impl<T: ParamsMetadata> __ParamMetaProbe<T> {
    pub fn param_infos(&self) -> Vec<ParamInfo> {
        T::param_infos()
    }
}

#[doc(hidden)]
pub trait __NoParamsMeta {
    fn param_infos(&self) -> Vec<ParamInfo> {
        Vec::new()
    }
}

impl<T> __NoParamsMeta for &__ParamMetaProbe<T> {}

// ── PrefixedExtract: core extraction trait for nested Params support ──

/// Trait for Params types that support prefixed extraction.
/// Generated by `#[derive(Params)]`. `FromRequestParts` delegates to this with empty prefix.
pub trait PrefixedExtract<S: Send + Sync>: Sized {
    fn extract_prefixed(
        parts: &mut crate::http::header::Parts,
        state: &S,
        prefix: &str,
    ) -> impl std::future::Future<Output = Result<Self, crate::http::response::Response>> + Send;
}

/// Build a prefixed query key. Returns borrowed `key` when prefix is empty (no allocation).
pub fn prefixed_key<'a>(prefix: &str, key: &'a str) -> std::borrow::Cow<'a, str> {
    if prefix.is_empty() {
        std::borrow::Cow::Borrowed(key)
    } else {
        std::borrow::Cow::Owned(format!("{prefix}.{key}"))
    }
}
