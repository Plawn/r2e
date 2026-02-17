use r2e_core::http::body::Body;
use r2e_core::http::Router;
use bytes::Bytes;
use http::header::{HeaderMap, HeaderName, IntoHeaderName, AUTHORIZATION, CONTENT_TYPE};
use http::{Method, Request, StatusCode};
use http_body_util::BodyExt;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use tower::util::ServiceExt;

/// In-process HTTP test client wrapping an Axum `Router`.
///
/// Uses `tower::ServiceExt::oneshot` to dispatch requests without binding
/// to a TCP port.
pub struct TestApp {
    router: Router,
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
}

/// Builder for constructing and sending a test HTTP request.
pub struct TestRequest<'a> {
    app: &'a TestApp,
    method: Method,
    path: String,
    headers: HeaderMap,
    body: Option<Vec<u8>>,
}

impl<'a> TestRequest<'a> {
    fn new(app: &'a TestApp, method: Method, path: &str) -> Self {
        Self {
            app,
            method,
            path: path.to_string(),
            headers: HeaderMap::new(),
            body: None,
        }
    }

    /// Add a Bearer token authorization header.
    pub fn bearer(mut self, token: &str) -> Self {
        self.headers.insert(
            AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        self
    }

    /// Add a custom header.
    pub fn header(mut self, name: impl IntoHeaderName, value: impl AsRef<str>) -> Self {
        self.headers.insert(
            name,
            value.as_ref().parse().unwrap(),
        );
        self
    }

    /// Set the request body as JSON. Also sets Content-Type to `application/json`.
    pub fn json(mut self, body: &impl Serialize) -> Self {
        self.body = Some(serde_json::to_vec(body).unwrap());
        self.headers.insert(
            CONTENT_TYPE,
            "application/json".parse().unwrap(),
        );
        self
    }

    /// Set a raw request body.
    pub fn body(mut self, body: impl Into<Vec<u8>>) -> Self {
        self.body = Some(body.into());
        self
    }

    /// Send the request and return the response.
    pub async fn send(self) -> TestResponse {
        let body = match self.body {
            Some(b) => Body::from(b),
            None => Body::empty(),
        };

        let mut builder = Request::builder()
            .method(self.method)
            .uri(&self.path);

        for (name, value) in &self.headers {
            builder = builder.header(name, value);
        }

        let request = builder.body(body).unwrap();

        let response = self
            .app
            .router
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

        TestResponse { status, headers, body }
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
                let end = rest.find(']').expect("unclosed bracket in JSON path");
                let index: usize = rest[start + 1..end]
                    .parse()
                    .expect("non-numeric array index in JSON path");
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
    let mut current = root.clone();
    for (i, token) in tokens.iter().enumerate() {
        current = match token {
            PathToken::Field(name) => current.get(name).cloned().unwrap_or(Value::Null),
            PathToken::Index(idx) => current.get(*idx).cloned().unwrap_or(Value::Null),
            PathToken::Len => {
                let len = match &current {
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
                Value::Number(serde_json::Number::from(len))
            }
        };
    }
    current
}

// ─── TestResponse ───

/// Response wrapper with status assertions, JSON-path assertions, and body helpers.
pub struct TestResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Bytes,
}

impl TestResponse {
    // ── Status assertions (common codes) ──

    /// Assert status is 200 OK.
    pub fn assert_ok(self) -> Self {
        self.assert_status(StatusCode::OK)
    }

    /// Assert status is 201 Created.
    pub fn assert_created(self) -> Self {
        self.assert_status(StatusCode::CREATED)
    }

    /// Assert status is 400 Bad Request.
    pub fn assert_bad_request(self) -> Self {
        self.assert_status(StatusCode::BAD_REQUEST)
    }

    /// Assert status is 401 Unauthorized.
    pub fn assert_unauthorized(self) -> Self {
        self.assert_status(StatusCode::UNAUTHORIZED)
    }

    /// Assert status is 403 Forbidden.
    pub fn assert_forbidden(self) -> Self {
        self.assert_status(StatusCode::FORBIDDEN)
    }

    /// Assert status is 404 Not Found.
    pub fn assert_not_found(self) -> Self {
        self.assert_status(StatusCode::NOT_FOUND)
    }

    /// Assert the response has a specific status code.
    pub fn assert_status(self, expected: StatusCode) -> Self {
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
    pub fn assert_json_path(self, path: &str, expected: impl Into<Value>) -> Self {
        let root: Value = self.json();
        let actual = resolve_path(&root, path);
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
        self,
        path: &str,
        predicate: impl FnOnce(&Value) -> bool,
    ) -> Self {
        let root: Value = self.json();
        let actual = resolve_path(&root, path);
        assert!(
            predicate(&actual),
            "JSON path \"{path}\" predicate failed\n  Value: {actual}\n  Body: {root}",
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
        let root: Value = self.json();
        let value = resolve_path(&root, path);
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
