//! Security headers plugin — adds common security-related HTTP headers to responses.
//!
//! # Default headers
//!
//! | Header | Value |
//! |--------|-------|
//! | `X-Content-Type-Options` | `nosniff` |
//! | `X-Frame-Options` | `DENY` |
//! | `Strict-Transport-Security` | `max-age=31536000; includeSubDomains` |
//! | `X-XSS-Protection` | `0` |
//! | `Referrer-Policy` | `strict-origin-when-cross-origin` |
//!
//! # Usage
//!
//! ```ignore
//! // Default headers
//! .with(SecureHeaders::default())
//!
//! // Custom configuration
//! .with(SecureHeaders::builder()
//!     .hsts(true)
//!     .hsts_max_age(63072000)
//!     .frame_options("SAMEORIGIN")
//!     .content_security_policy("default-src 'self'")
//!     .build())
//! ```

use axum::http::{HeaderName, HeaderValue};
use axum::response::Response;

use crate::builder::AppBuilder;
use crate::plugin::Plugin;

/// Security headers plugin.
///
/// Use `SecureHeaders::default()` for sensible defaults, or
/// `SecureHeaders::builder()` for custom configuration.
pub struct SecureHeaders {
    headers: Vec<(HeaderName, HeaderValue)>,
}

impl SecureHeaders {
    /// Create a builder for custom header configuration.
    pub fn builder() -> SecureHeadersBuilder {
        SecureHeadersBuilder::new()
    }

    /// Returns a reference to the collected headers.
    pub fn headers(&self) -> &[(HeaderName, HeaderValue)] {
        &self.headers
    }
}

impl Default for SecureHeaders {
    fn default() -> Self {
        SecureHeadersBuilder::new().build()
    }
}

impl Plugin for SecureHeaders {
    fn install<T: Clone + Send + Sync + 'static>(self, app: AppBuilder<T>) -> AppBuilder<T> {
        let headers = std::sync::Arc::new(self.headers);
        app.with_layer_fn(move |router| {
            let headers = headers.clone();
            router.layer(axum::middleware::from_fn(
                move |req: axum::extract::Request, next: crate::http::middleware::Next| {
                    let headers = headers.clone();
                    async move {
                        let mut response: Response = next.run(req).await;
                        for (name, value) in headers.iter() {
                            response.headers_mut().insert(name.clone(), value.clone());
                        }
                        response
                    }
                },
            ))
        })
    }
}

/// Builder for [`SecureHeaders`].
pub struct SecureHeadersBuilder {
    content_type_options: bool,
    frame_options: Option<String>,
    hsts: bool,
    hsts_max_age: u64,
    hsts_include_subdomains: bool,
    xss_protection: bool,
    referrer_policy: Option<String>,
    content_security_policy: Option<String>,
    permissions_policy: Option<String>,
}

impl SecureHeadersBuilder {
    fn new() -> Self {
        Self {
            content_type_options: true,
            frame_options: Some("DENY".to_string()),
            hsts: true,
            hsts_max_age: 31536000,
            hsts_include_subdomains: true,
            xss_protection: true,
            referrer_policy: Some("strict-origin-when-cross-origin".to_string()),
            content_security_policy: None,
            permissions_policy: None,
        }
    }

    /// Enable or disable `X-Content-Type-Options: nosniff`.
    pub fn content_type_options(mut self, enabled: bool) -> Self {
        self.content_type_options = enabled;
        self
    }

    /// Set the `X-Frame-Options` value (e.g. `"DENY"`, `"SAMEORIGIN"`).
    /// Pass `None` to omit the header.
    pub fn frame_options(mut self, value: impl Into<String>) -> Self {
        self.frame_options = Some(value.into());
        self
    }

    /// Disable `X-Frame-Options`.
    pub fn no_frame_options(mut self) -> Self {
        self.frame_options = None;
        self
    }

    /// Enable or disable `Strict-Transport-Security`.
    pub fn hsts(mut self, enabled: bool) -> Self {
        self.hsts = enabled;
        self
    }

    /// Set the `max-age` value for HSTS (in seconds).
    pub fn hsts_max_age(mut self, seconds: u64) -> Self {
        self.hsts_max_age = seconds;
        self
    }

    /// Enable or disable `includeSubDomains` in the HSTS header.
    pub fn hsts_include_subdomains(mut self, include: bool) -> Self {
        self.hsts_include_subdomains = include;
        self
    }

    /// Enable or disable the `X-XSS-Protection` header.
    pub fn xss_protection(mut self, enabled: bool) -> Self {
        self.xss_protection = enabled;
        self
    }

    /// Set `Referrer-Policy`.
    pub fn referrer_policy(mut self, value: impl Into<String>) -> Self {
        self.referrer_policy = Some(value.into());
        self
    }

    /// Set `Content-Security-Policy`.
    pub fn content_security_policy(mut self, value: impl Into<String>) -> Self {
        self.content_security_policy = Some(value.into());
        self
    }

    /// Set `Permissions-Policy`.
    pub fn permissions_policy(mut self, value: impl Into<String>) -> Self {
        self.permissions_policy = Some(value.into());
        self
    }

    /// Build the [`SecureHeaders`] plugin.
    pub fn build(self) -> SecureHeaders {
        let mut headers = Vec::new();

        if self.content_type_options {
            headers.push((
                HeaderName::from_static("x-content-type-options"),
                HeaderValue::from_static("nosniff"),
            ));
        }

        if let Some(ref fo) = self.frame_options {
            if let Ok(val) = HeaderValue::from_str(fo) {
                headers.push((HeaderName::from_static("x-frame-options"), val));
            }
        }

        if self.hsts {
            let value = if self.hsts_include_subdomains {
                format!("max-age={}; includeSubDomains", self.hsts_max_age)
            } else {
                format!("max-age={}", self.hsts_max_age)
            };
            if let Ok(val) = HeaderValue::from_str(&value) {
                headers.push((HeaderName::from_static("strict-transport-security"), val));
            }
        }

        if self.xss_protection {
            headers.push((
                HeaderName::from_static("x-xss-protection"),
                HeaderValue::from_static("0"),
            ));
        }

        if let Some(ref rp) = self.referrer_policy {
            if let Ok(val) = HeaderValue::from_str(rp) {
                headers.push((HeaderName::from_static("referrer-policy"), val));
            }
        }

        if let Some(ref csp) = self.content_security_policy {
            if let Ok(val) = HeaderValue::from_str(csp) {
                headers.push((HeaderName::from_static("content-security-policy"), val));
            }
        }

        if let Some(ref pp) = self.permissions_policy {
            if let Ok(val) = HeaderValue::from_str(pp) {
                headers.push((HeaderName::from_static("permissions-policy"), val));
            }
        }

        SecureHeaders { headers }
    }
}
