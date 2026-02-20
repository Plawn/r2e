use crate::http::response::{IntoResponse, Response};
use crate::http::{Json, StatusCode};

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

impl IntoResponse for ParamError {
    fn into_response(self) -> Response {
        let body = serde_json::json!({ "error": self.message });
        (StatusCode::BAD_REQUEST, Json(body)).into_response()
    }
}

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
