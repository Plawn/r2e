use r2e_core::http::body::Body;
use r2e_core::http::Router;
use bytes::Bytes;
use http::header::{AUTHORIZATION, CONTENT_TYPE};
use http::{Method, Request, StatusCode};
use http_body_util::BodyExt;
use serde::de::DeserializeOwned;
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

    /// Send an arbitrary request.
    pub async fn send(&self, request: Request<Body>) -> TestResponse {
        let response = self
            .router
            .clone()
            .oneshot(request)
            .await
            .expect("failed to send request");

        let status = response.status();
        let body = response
            .into_body()
            .collect()
            .await
            .expect("failed to read response body")
            .to_bytes();

        TestResponse { status, body }
    }

    /// Send a GET request to the given path.
    pub async fn get(&self, path: &str) -> TestResponse {
        let req = Request::builder()
            .method(Method::GET)
            .uri(path)
            .body(Body::empty())
            .unwrap();
        self.send(req).await
    }

    /// Send a GET request with a Bearer token.
    pub async fn get_authenticated(&self, path: &str, token: &str) -> TestResponse {
        let req = Request::builder()
            .method(Method::GET)
            .uri(path)
            .header(AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        self.send(req).await
    }

    /// Send a POST request with a JSON body.
    pub async fn post_json(&self, path: &str, body: &impl serde::Serialize) -> TestResponse {
        let json = serde_json::to_vec(body).unwrap();
        let req = Request::builder()
            .method(Method::POST)
            .uri(path)
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(json))
            .unwrap();
        self.send(req).await
    }

    /// Send a POST request with a JSON body and a Bearer token.
    pub async fn post_json_authenticated(
        &self,
        path: &str,
        body: &impl serde::Serialize,
        token: &str,
    ) -> TestResponse {
        let json = serde_json::to_vec(body).unwrap();
        let req = Request::builder()
            .method(Method::POST)
            .uri(path)
            .header(CONTENT_TYPE, "application/json")
            .header(AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::from(json))
            .unwrap();
        self.send(req).await
    }

    /// Send a PUT request with a JSON body and a Bearer token.
    pub async fn put_json_authenticated(
        &self,
        path: &str,
        body: &impl serde::Serialize,
        token: &str,
    ) -> TestResponse {
        let json = serde_json::to_vec(body).unwrap();
        let req = Request::builder()
            .method(Method::PUT)
            .uri(path)
            .header(CONTENT_TYPE, "application/json")
            .header(AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::from(json))
            .unwrap();
        self.send(req).await
    }

    /// Send a DELETE request with a Bearer token.
    pub async fn delete_authenticated(&self, path: &str, token: &str) -> TestResponse {
        let req = Request::builder()
            .method(Method::DELETE)
            .uri(path)
            .header(AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        self.send(req).await
    }
}

/// Response wrapper with assertion helpers.
pub struct TestResponse {
    pub status: StatusCode,
    pub body: Bytes,
}

impl TestResponse {
    /// Assert status is 200 OK. Returns `self` for chaining.
    pub fn assert_ok(self) -> Self {
        assert_eq!(self.status, StatusCode::OK, "Expected 200 OK, got {}", self.status);
        self
    }

    /// Assert status is 201 Created. Returns `self` for chaining.
    pub fn assert_created(self) -> Self {
        assert_eq!(self.status, StatusCode::CREATED, "Expected 201 Created, got {}", self.status);
        self
    }

    /// Assert status is 400 Bad Request.
    pub fn assert_bad_request(self) -> Self {
        assert_eq!(self.status, StatusCode::BAD_REQUEST, "Expected 400 Bad Request, got {}", self.status);
        self
    }

    /// Assert status is 401 Unauthorized.
    pub fn assert_unauthorized(self) -> Self {
        assert_eq!(self.status, StatusCode::UNAUTHORIZED, "Expected 401 Unauthorized, got {}", self.status);
        self
    }

    /// Assert status is 403 Forbidden.
    pub fn assert_forbidden(self) -> Self {
        assert_eq!(self.status, StatusCode::FORBIDDEN, "Expected 403 Forbidden, got {}", self.status);
        self
    }

    /// Assert status is 404 Not Found.
    pub fn assert_not_found(self) -> Self {
        assert_eq!(self.status, StatusCode::NOT_FOUND, "Expected 404 Not Found, got {}", self.status);
        self
    }

    /// Assert the response has a specific status code.
    pub fn assert_status(self, expected: StatusCode) -> Self {
        assert_eq!(self.status, expected, "Expected {expected}, got {}", self.status);
        self
    }

    /// Deserialize the response body as JSON.
    pub fn json<T: DeserializeOwned>(&self) -> T {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|e| panic!("Failed to parse JSON: {e}\nBody: {}", self.text()))
    }

    /// Return the response body as a UTF-8 string.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}
