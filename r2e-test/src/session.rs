use crate::app::{execute_request, parse_set_cookie, RequestParts, TestResponse};
use crate::TestApp;
use http::header::{HeaderMap, IntoHeaderName, AUTHORIZATION, CONTENT_TYPE, COOKIE, SET_COOKIE};
use http::Method;
use serde::Serialize;
use std::cell::RefCell;
use std::collections::BTreeMap;

/// A test session that persists cookies across requests.
///
/// Useful for testing login flows, CSRF tokens, or any stateful
/// cookie-based interactions.
///
/// ```ignore
/// let session = app.session();
/// // Login — cookies from Set-Cookie are captured
/// session.post("/login").form(&[("user", "admin"), ("pass", "secret")]).send().await;
/// // Subsequent requests automatically include captured cookies
/// session.get("/dashboard").send().await.assert_ok();
/// ```
pub struct TestSession<'a> {
    app: &'a TestApp,
    cookies: RefCell<BTreeMap<String, String>>,
    default_headers: HeaderMap,
}

impl<'a> TestSession<'a> {
    /// Create a new session for the given `TestApp`.
    pub fn new(app: &'a TestApp) -> Self {
        Self {
            app,
            cookies: RefCell::new(BTreeMap::new()),
            default_headers: HeaderMap::new(),
        }
    }

    /// Set a default Bearer token for all requests in this session.
    pub fn with_bearer(mut self, token: &str) -> Self {
        self.default_headers.insert(
            AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        self
    }

    /// Set a default header for all requests in this session.
    pub fn with_default_header(mut self, name: impl IntoHeaderName, value: &str) -> Self {
        self.default_headers.insert(name, value.parse().unwrap());
        self
    }

    /// Manually set a cookie in the session jar.
    pub fn set_cookie(&self, name: &str, value: &str) {
        self.cookies.borrow_mut().insert(name.to_string(), value.to_string());
    }

    /// Remove a cookie from the session jar.
    pub fn remove_cookie(&self, name: &str) {
        self.cookies.borrow_mut().remove(name);
    }

    /// Clear all cookies from the session jar.
    pub fn clear_cookies(&self) {
        self.cookies.borrow_mut().clear();
    }

    /// Get a cookie value from the session jar.
    pub fn cookie(&self, name: &str) -> Option<String> {
        self.cookies.borrow().get(name).cloned()
    }

    /// Start building a GET request.
    pub fn get(&self, path: &str) -> SessionRequest<'_, 'a> {
        SessionRequest::new(self, Method::GET, path)
    }

    /// Start building a POST request.
    pub fn post(&self, path: &str) -> SessionRequest<'_, 'a> {
        SessionRequest::new(self, Method::POST, path)
    }

    /// Start building a PUT request.
    pub fn put(&self, path: &str) -> SessionRequest<'_, 'a> {
        SessionRequest::new(self, Method::PUT, path)
    }

    /// Start building a PATCH request.
    pub fn patch(&self, path: &str) -> SessionRequest<'_, 'a> {
        SessionRequest::new(self, Method::PATCH, path)
    }

    /// Start building a DELETE request.
    pub fn delete(&self, path: &str) -> SessionRequest<'_, 'a> {
        SessionRequest::new(self, Method::DELETE, path)
    }

    /// Start building a request with an arbitrary HTTP method.
    pub fn request(&self, method: Method, path: &str) -> SessionRequest<'_, 'a> {
        SessionRequest::new(self, method, path)
    }

    /// Build the Cookie header from the session jar.
    fn cookie_header_value(&self) -> Option<String> {
        let cookies = self.cookies.borrow();
        if cookies.is_empty() {
            return None;
        }
        let mut result = String::new();
        for (i, (k, v)) in cookies.iter().enumerate() {
            if i > 0 {
                result.push_str("; ");
            }
            result.push_str(k);
            result.push('=');
            result.push_str(v);
        }
        Some(result)
    }

    /// Update the session jar from Set-Cookie response headers.
    fn capture_cookies(&self, headers: &HeaderMap) {
        let mut jar = self.cookies.borrow_mut();
        for value in headers.get_all(SET_COOKIE) {
            if let Ok(s) = value.to_str() {
                if let Some((name, val)) = parse_set_cookie(s) {
                    jar.insert(name.to_string(), val.to_string());
                }
            }
        }
    }
}

/// Builder for constructing and sending a request within a `TestSession`.
pub struct SessionRequest<'s, 'a> {
    session: &'s TestSession<'a>,
    parts: RequestParts,
}

impl<'s, 'a> SessionRequest<'s, 'a> {
    fn new(session: &'s TestSession<'a>, method: Method, path: &str) -> Self {
        Self {
            session,
            parts: RequestParts::new(method, path),
        }
    }

    impl_request_builders!();

    /// Send the request and return the response.
    ///
    /// Cookies from the session jar are included automatically.
    /// `Set-Cookie` headers from the response update the session jar.
    pub async fn send(self) -> TestResponse {
        let RequestParts { method, path, headers, body, queries } = self.parts;

        // Start with session defaults, override with per-request headers
        let mut merged = self.session.default_headers.clone();
        for (name, value) in headers {
            if let Some(name) = name {
                merged.insert(name, value);
            }
        }

        // Add cookies from the session jar
        if let Some(cookie_value) = self.session.cookie_header_value() {
            merged.insert(COOKIE, cookie_value.parse().unwrap());
        }

        let parts = RequestParts {
            method,
            path,
            headers: merged,
            body,
            queries,
        };

        let request = parts.into_request();
        let response = execute_request(&self.session.app.router, request).await;

        // Capture Set-Cookie headers into the session jar
        self.session.capture_cookies(&response.headers);

        response
    }
}
