use r2e_core::http::body::Body;
use r2e_core::http::Router;
use bytes::Bytes;
use http::header::{HeaderMap, HeaderName, IntoHeaderName, AUTHORIZATION, CONTENT_TYPE, COOKIE, SET_COOKIE};
use http::{Method, Request, StatusCode};
use http_body_util::BodyExt;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use std::sync::OnceLock;
use tower::util::ServiceExt;

// ─── Shared request building ───

/// Common request fields shared between `TestRequest` and `SessionRequest`.
pub(crate) struct RequestParts {
    pub(crate) method: Method,
    pub(crate) path: String,
    pub(crate) headers: HeaderMap,
    pub(crate) body: Option<Vec<u8>>,
    pub(crate) queries: Vec<(String, String)>,
}

impl RequestParts {
    pub(crate) fn new(method: Method, path: &str) -> Self {
        Self {
            method,
            path: path.to_string(),
            headers: HeaderMap::new(),
            body: None,
            queries: Vec::new(),
        }
    }

    /// Build an HTTP request from these parts.
    pub(crate) fn into_request(self) -> Request<Body> {
        let uri = build_uri(&self.path, &self.queries);
        let body = match self.body {
            Some(b) => Body::from(b),
            None => Body::empty(),
        };
        let mut builder = Request::builder()
            .method(self.method)
            .uri(&uri);
        for (name, value) in &self.headers {
            builder = builder.header(name, value);
        }
        builder.body(body).unwrap()
    }
}

/// Parse a `Set-Cookie` header value into `(name, value)`.
pub(crate) fn parse_set_cookie(header_value: &str) -> Option<(&str, &str)> {
    let eq_pos = header_value.find('=')?;
    let name = header_value[..eq_pos].trim();
    let rest = &header_value[eq_pos + 1..];
    let value = rest.split(';').next().unwrap_or("");
    Some((name, value))
}

/// Builder methods shared between `TestRequest` and `SessionRequest`.
///
/// Requires the implementing type to have a `parts: RequestParts` field.
macro_rules! impl_request_builders {
    () => {
        /// Add a Bearer token authorization header.
        pub fn bearer(mut self, token: &str) -> Self {
            self.parts.headers.insert(
                AUTHORIZATION,
                format!("Bearer {token}").parse().unwrap(),
            );
            self
        }

        /// Add a custom header.
        pub fn header(mut self, name: impl IntoHeaderName, value: impl AsRef<str>) -> Self {
            self.parts.headers.insert(
                name,
                value.as_ref().parse().unwrap(),
            );
            self
        }

        /// Set the request body as JSON. Also sets Content-Type to `application/json`.
        pub fn json(mut self, body: &impl Serialize) -> Self {
            self.parts.body = Some(serde_json::to_vec(body).unwrap());
            self.parts.headers.insert(
                CONTENT_TYPE,
                "application/json".parse().unwrap(),
            );
            self
        }

        /// Set a raw request body.
        pub fn body(mut self, body: impl Into<Vec<u8>>) -> Self {
            self.parts.body = Some(body.into());
            self
        }

        /// Set the request body as URL-encoded form data.
        pub fn form(mut self, fields: &[(&str, &str)]) -> Self {
            let body = form_urlencoded::Serializer::new(String::new())
                .extend_pairs(fields)
                .finish();
            self.parts.body = Some(body.into_bytes());
            self.parts.headers.insert(
                CONTENT_TYPE,
                "application/x-www-form-urlencoded".parse().unwrap(),
            );
            self
        }

        /// Add a cookie to the request.
        pub fn cookie(mut self, name: &str, value: &str) -> Self {
            let new_pair = format!("{name}={value}");
            if let Some(existing) = self.parts.headers.get(COOKIE) {
                let existing = existing.to_str().unwrap();
                let combined = format!("{existing}; {new_pair}");
                self.parts.headers.insert(COOKIE, combined.parse().unwrap());
            } else {
                self.parts.headers.insert(COOKIE, new_pair.parse().unwrap());
            }
            self
        }

        /// Add a query parameter.
        pub fn query(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
            self.parts.queries.push((key.into(), value.into()));
            self
        }

        /// Add multiple query parameters.
        pub fn queries(mut self, params: impl IntoIterator<Item = (impl Into<String>, impl Into<String>)>) -> Self {
            for (k, v) in params {
                self.parts.queries.push((k.into(), v.into()));
            }
            self
        }
    };
}

// ─── TestApp ───

/// In-process HTTP test client wrapping an Axum `Router`.
///
/// Uses `tower::ServiceExt::oneshot` to dispatch requests without binding
/// to a TCP port.
pub struct TestApp {
    pub(crate) router: Router,
}

impl TestApp {
    /// Create a `TestApp` from an assembled `axum::Router`.
    pub fn new(router: Router) -> Self {
        Self { router }
    }

    /// Create a `TestApp` from an `AppBuilder` by calling `.build()`.
    pub fn from_builder(builder: r2e_core::AppBuilder<impl Clone + Send + Sync + 'static>) -> Self {
        Self::new(builder.build())
    }

    /// Start building a GET request.
    pub fn get(&self, path: &str) -> TestRequest<'_> {
        TestRequest::new(self, Method::GET, path)
    }

    /// Start building a POST request.
    pub fn post(&self, path: &str) -> TestRequest<'_> {
        TestRequest::new(self, Method::POST, path)
    }

    /// Start building a PUT request.
    pub fn put(&self, path: &str) -> TestRequest<'_> {
        TestRequest::new(self, Method::PUT, path)
    }

    /// Start building a PATCH request.
    pub fn patch(&self, path: &str) -> TestRequest<'_> {
        TestRequest::new(self, Method::PATCH, path)
    }

    /// Start building a DELETE request.
    pub fn delete(&self, path: &str) -> TestRequest<'_> {
        TestRequest::new(self, Method::DELETE, path)
    }

    /// Start building a request with an arbitrary HTTP method.
    pub fn request(&self, method: Method, path: &str) -> TestRequest<'_> {
        TestRequest::new(self, method, path)
    }

    /// Create a `TestSession` that persists cookies across requests.
    pub fn session(&self) -> crate::session::TestSession<'_> {
        crate::session::TestSession::new(self)
    }
}

// ─── TestRequest ───

/// Builder for constructing and sending a test HTTP request.
pub struct TestRequest<'a> {
    app: &'a TestApp,
    parts: RequestParts,
}

impl<'a> TestRequest<'a> {
    fn new(app: &'a TestApp, method: Method, path: &str) -> Self {
        Self {
            app,
            parts: RequestParts::new(method, path),
        }
    }

    impl_request_builders!();

    /// Send the request and return the response.
    pub async fn send(self) -> TestResponse {
        let request = self.parts.into_request();
        execute_request(&self.app.router, request).await
    }
}

// ─── Shared helpers ───

/// Build a URI by merging path and additional query params.
pub(crate) fn build_uri(path: &str, queries: &[(String, String)]) -> String {
    if queries.is_empty() {
        return path.to_string();
    }

    let mut serializer = form_urlencoded::Serializer::new(String::new());
    for (k, v) in queries {
        serializer.append_pair(k, v);
    }
    let new_query = serializer.finish();

    if path.contains('?') {
        format!("{path}&{new_query}")
    } else {
        format!("{path}?{new_query}")
    }
}

/// Execute an HTTP request against a router and return a `TestResponse`.
///
/// This is used by both `TestRequest::send()` and `SessionRequest::send()`.
pub(crate) async fn execute_request(router: &Router, request: Request<Body>) -> TestResponse {
    let response = router
        .clone()
        .oneshot(request)
        .await
        .expect("failed to send request");

    let status = response.status();
    let headers = response.headers().clone();
    let body = response
        .into_body()
        .collect()
        .await
        .expect("failed to read response body")
        .to_bytes();

    TestResponse {
        status,
        headers,
        body,
        json_cache: OnceLock::new(),
    }
}

// ─── JSON path resolution ───

#[derive(Debug)]
pub enum PathToken {
    Field(String),
    Index(usize),
    Len,
}

pub fn tokenize_path(path: &str) -> Vec<PathToken> {
    let mut tokens = Vec::new();
    for segment in path.split('.') {
        if segment.is_empty() {
            continue;
        }
        if segment == "len()" || segment == "size()" {
            tokens.push(PathToken::Len);
            continue;
        }
        if let Some(bracket_pos) = segment.find('[') {
            let field = &segment[..bracket_pos];
            if !field.is_empty() {
                tokens.push(PathToken::Field(field.to_string()));
            }
            let mut rest = &segment[bracket_pos..];
            while let Some(start) = rest.find('[') {
                let end = rest.find(']').unwrap_or_else(|| {
                    panic!("unclosed bracket in JSON path: \"{path}\"")
                });
                let index: usize = rest[start + 1..end]
                    .parse()
                    .unwrap_or_else(|_| panic!("non-numeric array index in JSON path: \"{path}\""));
                tokens.push(PathToken::Index(index));
                rest = &rest[end + 1..];
            }
        } else {
            tokens.push(PathToken::Field(segment.to_string()));
        }
    }
    tokens
}

pub fn resolve_path(root: &Value, path: &str) -> Value {
    let tokens = tokenize_path(path);
    let mut current: &Value = root;
    for (i, token) in tokens.iter().enumerate() {
        match token {
            PathToken::Field(name) => {
                current = current.get(name).unwrap_or(&Value::Null);
            }
            PathToken::Index(idx) => {
                current = current.get(*idx).unwrap_or(&Value::Null);
            }
            PathToken::Len => {
                let len = match current {
                    Value::Array(a) => a.len(),
                    Value::Object(o) => o.len(),
                    Value::String(s) => s.len(),
                    other => {
                        let consumed: Vec<_> = tokens[..i]
                            .iter()
                            .map(|t| format!("{t:?}"))
                            .collect();
                        panic!(
                            "len() applied to non-collection at path segment {}: got {}\nResolved tokens so far: {:?}",
                            i, other, consumed,
                        );
                    }
                };
                return Value::Number(serde_json::Number::from(len));
            }
        }
    }
    current.clone()
}

// ─── JSON matching helpers ───

/// Check if `actual` JSON contains all keys/values from `expected`.
///
/// - Objects: every key in `expected` must exist in `actual` with a matching value.
/// - Arrays: every element in `expected` must match at least one element in `actual`.
/// - Scalars: exact equality.
pub fn json_contains(actual: &Value, expected: &Value) -> bool {
    match (actual, expected) {
        (Value::Object(a), Value::Object(e)) => {
            e.iter().all(|(k, v)| a.get(k).map_or(false, |av| json_contains(av, v)))
        }
        (Value::Array(a), Value::Array(e)) => {
            e.iter().all(|ev| a.iter().any(|av| json_contains(av, ev)))
        }
        _ => actual == expected,
    }
}

// ─── JSON shape validation helpers ───

fn json_type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn json_shape_errors(actual: &Value, schema: &Value, path: &str) -> Vec<String> {
    let mut errors = Vec::new();
    match (actual, schema) {
        (Value::Object(a), Value::Object(s)) => {
            for (k, sv) in s {
                let child_path = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}.{k}")
                };
                match a.get(k) {
                    Some(av) => errors.extend(json_shape_errors(av, sv, &child_path)),
                    None => errors.push(format!("missing key \"{child_path}\"")),
                }
            }
        }
        (Value::Array(a), Value::Array(s)) => {
            if let Some(item_schema) = s.first() {
                for (i, av) in a.iter().enumerate() {
                    let child_path = format!("{path}[{i}]");
                    errors.extend(json_shape_errors(av, item_schema, &child_path));
                }
            }
        }
        _ => {
            let expected_type = json_type_name(schema);
            let actual_type = json_type_name(actual);
            if expected_type != actual_type {
                let loc = if path.is_empty() { "<root>" } else { path };
                errors.push(format!(
                    "at \"{loc}\": expected {expected_type}, got {actual_type}"
                ));
            }
        }
    }
    errors
}

// ─── TestResponse ───

/// Response wrapper with status assertions, JSON-path assertions, and body helpers.
pub struct TestResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Bytes,
    json_cache: OnceLock<Value>,
}

impl TestResponse {
    /// Get the cached JSON `Value`, parsing on first access.
    fn json_value(&self) -> &Value {
        self.json_cache.get_or_init(|| {
            serde_json::from_slice(&self.body)
                .unwrap_or_else(|e| panic!("Failed to parse JSON: {e}\nBody: {}", self.text()))
        })
    }

    // ── Status assertions (common codes) ──

    /// Assert status is 200 OK.
    pub fn assert_ok(&self) -> &Self {
        self.assert_status(StatusCode::OK)
    }

    /// Assert status is 201 Created.
    pub fn assert_created(&self) -> &Self {
        self.assert_status(StatusCode::CREATED)
    }

    /// Assert status is 204 No Content.
    pub fn assert_no_content(&self) -> &Self {
        self.assert_status(StatusCode::NO_CONTENT)
    }

    /// Assert status is 400 Bad Request.
    pub fn assert_bad_request(&self) -> &Self {
        self.assert_status(StatusCode::BAD_REQUEST)
    }

    /// Assert status is 401 Unauthorized.
    pub fn assert_unauthorized(&self) -> &Self {
        self.assert_status(StatusCode::UNAUTHORIZED)
    }

    /// Assert status is 403 Forbidden.
    pub fn assert_forbidden(&self) -> &Self {
        self.assert_status(StatusCode::FORBIDDEN)
    }

    /// Assert status is 404 Not Found.
    pub fn assert_not_found(&self) -> &Self {
        self.assert_status(StatusCode::NOT_FOUND)
    }

    /// Assert status is 409 Conflict.
    pub fn assert_conflict(&self) -> &Self {
        self.assert_status(StatusCode::CONFLICT)
    }

    /// Assert status is 422 Unprocessable Entity.
    pub fn assert_unprocessable(&self) -> &Self {
        self.assert_status(StatusCode::UNPROCESSABLE_ENTITY)
    }

    /// Assert status is 429 Too Many Requests.
    pub fn assert_too_many_requests(&self) -> &Self {
        self.assert_status(StatusCode::TOO_MANY_REQUESTS)
    }

    /// Assert status is 500 Internal Server Error.
    pub fn assert_internal_server_error(&self) -> &Self {
        self.assert_status(StatusCode::INTERNAL_SERVER_ERROR)
    }

    /// Assert the response has a specific status code.
    pub fn assert_status(&self, expected: StatusCode) -> &Self {
        assert_eq!(
            self.status,
            expected,
            "Expected {expected}, got {}\nBody: {}",
            self.status,
            self.text()
        );
        self
    }

    // ── JSON-path assertions ──

    /// Assert that a JSON path resolves to the expected value.
    ///
    /// Supports dot-separated fields, array indices, and `len()`/`size()`:
    /// ```ignore
    /// resp.assert_json_path("users[0].name", "Alice")
    ///     .assert_json_path("users.len()", 2)
    ///     .assert_json_path("meta.page", 1)
    ///     .assert_json_path("active", true);
    /// ```
    pub fn assert_json_path(&self, path: &str, expected: impl Into<Value>) -> &Self {
        let root = self.json_value();
        let actual = resolve_path(root, path);
        let expected = expected.into();
        assert_eq!(
            actual, expected,
            "JSON path \"{path}\" assertion failed\n  Expected: {expected}\n  Actual:   {actual}\n  Body: {root}",
        );
        self
    }

    /// Assert that a JSON path satisfies a predicate.
    ///
    /// ```ignore
    /// resp.assert_json_path_fn("tags", |v| v.as_array().unwrap().contains(&json!("rust")));
    /// ```
    pub fn assert_json_path_fn(
        &self,
        path: &str,
        predicate: impl FnOnce(&Value) -> bool,
    ) -> &Self {
        let root = self.json_value();
        let actual = resolve_path(root, path);
        assert!(
            predicate(&actual),
            "JSON path \"{path}\" predicate failed\n  Value: {actual}\n  Body: {root}",
        );
        self
    }

    /// Assert that the response JSON contains all keys/values from `expected`.
    pub fn assert_json_contains(&self, expected: Value) -> &Self {
        let root = self.json_value();
        assert!(
            json_contains(root, &expected),
            "JSON contains assertion failed\n  Expected (subset): {expected}\n  Actual: {root}",
        );
        self
    }

    /// Assert that a JSON path value contains the given item (partial match).
    pub fn assert_json_path_contains(&self, path: &str, item: impl Into<Value>) -> &Self {
        let root = self.json_value();
        let actual = resolve_path(root, path);
        let item = item.into();
        assert!(
            json_contains(&actual, &item),
            "JSON path \"{path}\" contains assertion failed\n  Expected (subset): {item}\n  Actual: {actual}\n  Body: {root}",
        );
        self
    }

    /// Assert that the response JSON matches the expected shape (type structure).
    ///
    /// Schema values serve as type exemplars: `0` means "number", `""` means "string",
    /// `true` means "boolean", `[]` checks array element types, `{}` checks nested keys.
    ///
    /// ```ignore
    /// resp.assert_json_shape(json!({
    ///     "id": 0,
    ///     "name": "",
    ///     "active": true,
    ///     "tags": [""]
    /// }));
    /// ```
    pub fn assert_json_shape(&self, schema: Value) -> &Self {
        let root = self.json_value();
        let errors = json_shape_errors(root, &schema, "");
        assert!(
            errors.is_empty(),
            "JSON shape assertion failed:\n{}\n  Body: {root}",
            errors.join("\n"),
        );
        self
    }

    // ── Header assertions ──

    /// Assert that a response header has the expected value.
    pub fn assert_header(&self, name: impl AsRef<str>, expected: &str) -> &Self {
        let name_str = name.as_ref();
        let actual = self.header(name_str);
        assert_eq!(
            actual,
            Some(expected),
            "Header \"{name_str}\" assertion failed\n  Expected: {expected}\n  Actual: {actual:?}",
        );
        self
    }

    /// Assert that a response header exists (regardless of value).
    pub fn assert_header_exists(&self, name: impl AsRef<str>) -> &Self {
        let name_str = name.as_ref();
        assert!(
            self.header(name_str).is_some(),
            "Expected header \"{name_str}\" to exist, but it was not present",
        );
        self
    }

    /// Extract and deserialize a value at a JSON path.
    ///
    /// ```ignore
    /// let name: String = resp.json_path("users[0].name");
    /// let count: usize = resp.json_path("items.len()");
    /// ```
    pub fn json_path<T: DeserializeOwned>(&self, path: &str) -> T {
        let root = self.json_value();
        let value = resolve_path(root, path);
        serde_json::from_value(value.clone()).unwrap_or_else(|e| {
            panic!(
                "Failed to deserialize JSON path \"{path}\": {e}\n  Value: {value}\n  Body: {root}"
            )
        })
    }

    // ── Header access ──

    /// Get a response header value by name.
    pub fn header(&self, name: impl AsRef<str>) -> Option<&str> {
        let name: HeaderName = name.as_ref().parse().ok()?;
        self.headers.get(name).and_then(|v| v.to_str().ok())
    }

    // ── Cookie access ──

    /// Get a cookie value from the `Set-Cookie` response headers by name.
    pub fn cookie(&self, name: &str) -> Option<String> {
        self.headers
            .get_all(SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .find_map(|cookie_str| {
                let (n, v) = parse_set_cookie(cookie_str)?;
                if n == name { Some(v.to_string()) } else { None }
            })
    }

    /// Return all `Set-Cookie` header values as raw strings.
    pub fn cookies(&self) -> Vec<&str> {
        self.headers
            .get_all(SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .collect()
    }

    // ── Body helpers ──

    /// Deserialize the entire response body as JSON.
    pub fn json<T: DeserializeOwned>(&self) -> T {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|e| panic!("Failed to parse JSON: {e}\nBody: {}", self.text()))
    }

    /// Return the response body as a UTF-8 string.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}
