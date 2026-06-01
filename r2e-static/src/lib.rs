//! Embedded static file serving with SPA support for R2E.
//!
//! This crate provides an [`EmbeddedFrontend`] plugin that serves static files
//! embedded in the binary via [`rust_embed`], with optional SPA fallback support.
//!
//! # Features
//!
//! - **SPA fallback** — unknown routes serve `index.html` (configurable)
//! - **ETag + 304** — conditional requests via `If-None-Match`
//! - **Pre-compressed variants** — serves `.br`/`.gz` based on `Accept-Encoding`
//! - **Range requests** — `206 Partial Content` for byte-range requests
//! - **Immutable cache** — hashed assets get long-lived cache headers
//! - **SPA-aware caching** — fallback file defaults to `no-cache`
//!
//! # Quick start
//!
//! ```ignore
//! #[derive(rust_embed::Embed)]
//! #[folder = "frontend/dist"]
//! struct Assets;
//!
//! app.with(EmbeddedFrontend::new::<Assets>())
//! ```
//!
//! # Builder API
//!
//! ```ignore
//! app.with(EmbeddedFrontend::builder::<Assets>()
//!     .exclude_prefix("api/")
//!     .immutable_prefix("assets/")
//!     .spa_fallback(true)
//!     .fallback_file("index.html")
//!     .compression(true)
//!     .build())
//! ```

use std::borrow::Cow;
use std::sync::Arc;

use r2e_core::http::header::{
    HeaderMap, ACCEPT_ENCODING, ACCEPT_RANGES, CACHE_CONTROL, CONTENT_ENCODING,
    CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, ETAG, IF_NONE_MATCH, RANGE, VARY,
};
use r2e_core::http::response::IntoResponse;
use r2e_core::http::{Body, Response, StatusCode};
pub use rust_embed;

// ── Fast hex encoding ──────────────────────────────────────────────────────

const HEX_LUT: &[u8; 16] = b"0123456789abcdef";

fn bytes_to_hex(bytes: &[u8]) -> String {
    let mut hex = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        hex.push(HEX_LUT[(b >> 4) as usize] as char);
        hex.push(HEX_LUT[(b & 0xf) as usize] as char);
    }
    hex
}

// ── File server abstraction ────────────────────────────────────────────────

/// Data returned by a [`FileServer`] for a given path.
pub struct EmbeddedFileData {
    /// The file contents — either borrowed (compile-time embed) or owned.
    pub data: Cow<'static, [u8]>,
    /// Optional SHA-256 hash of the file contents (for ETag).
    pub hash: Option<String>,
}

/// Object-safe trait abstracting over `rust_embed::Embed`.
pub trait FileServer: Send + Sync + 'static {
    /// Retrieve a file by its embedded path.
    fn get_file(&self, path: &str) -> Option<EmbeddedFileData>;
}

/// Adapter that wraps a `rust_embed::Embed` type into a [`FileServer`].
struct EmbedAdapter<E: rust_embed::Embed>(std::marker::PhantomData<E>);

impl<E: rust_embed::Embed + Send + Sync + 'static> FileServer for EmbedAdapter<E> {
    fn get_file(&self, path: &str) -> Option<EmbeddedFileData> {
        let file = E::get(path)?;
        let hash = Some(bytes_to_hex(&file.metadata.sha256_hash()));
        Some(EmbeddedFileData {
            data: file.data,
            hash,
        })
    }
}

// ── Range parsing ──────────────────────────────────────────────────────────

struct ByteRange {
    start: u64,
    end_inclusive: u64,
}

fn parse_byte_range(header_val: &str, file_size: u64) -> Option<ByteRange> {
    let s = header_val.strip_prefix("bytes=")?;
    if s.contains(',') || file_size == 0 {
        return None;
    }

    if let Some(suffix) = s.strip_prefix('-') {
        let n: u64 = suffix.parse().ok()?;
        if n == 0 || n > file_size {
            return None;
        }
        Some(ByteRange {
            start: file_size - n,
            end_inclusive: file_size - 1,
        })
    } else if s.ends_with('-') {
        let start: u64 = s.trim_end_matches('-').parse().ok()?;
        if start >= file_size {
            return None;
        }
        Some(ByteRange {
            start,
            end_inclusive: file_size - 1,
        })
    } else {
        let (start_str, end_str) = s.split_once('-')?;
        let start: u64 = start_str.parse().ok()?;
        let end: u64 = end_str.parse().ok()?;
        if start > end || start >= file_size {
            return None;
        }
        Some(ByteRange {
            start,
            end_inclusive: end.min(file_size - 1),
        })
    }
}

// ── Encoding / ETag helpers ────────────────────────────────────────────────

fn accepts_encoding(accept: &str, encoding: &str) -> bool {
    accept.split(',').any(|part| {
        let mut iter = part.split(';');
        let name = iter.next().unwrap_or("").trim();
        let matches = name.eq_ignore_ascii_case(encoding) || name == "*";
        if !matches {
            return false;
        }
        for param in iter {
            let param = param.trim();
            if let Some(q) = param.strip_prefix("q=") {
                if let Ok(val) = q.trim().parse::<f32>() {
                    return val > 0.0;
                }
            }
        }
        true
    })
}

fn etag_matches(if_none_match: &str, etag: &str) -> bool {
    if_none_match.split(',').any(|candidate| {
        let trimmed = candidate.trim();
        let val = trimmed.strip_prefix("W/").unwrap_or(trimmed);
        val == etag
    })
}

// ── Configuration & Builder ────────────────────────────────────────────────

/// Plugin that serves embedded static files as an Axum fallback handler.
///
/// See module-level docs for usage.
pub struct EmbeddedFrontend {
    file_server: Box<dyn FileServer>,
    config: StaticConfig,
}

#[derive(Clone)]
struct StaticConfig {
    excluded_prefixes: Vec<String>,
    spa_fallback: bool,
    fallback_file: String,
    fallback_cache_control: String,
    immutable_prefix: Option<String>,
    immutable_cache_control: String,
    default_cache_control: String,
    base_path: Option<String>,
    compression: bool,
}

impl Default for StaticConfig {
    fn default() -> Self {
        Self {
            excluded_prefixes: vec!["api/".to_string()],
            spa_fallback: true,
            fallback_file: "index.html".to_string(),
            fallback_cache_control: "no-cache".to_string(),
            immutable_prefix: Some("assets/".to_string()),
            immutable_cache_control: "public, max-age=31536000, immutable".to_string(),
            default_cache_control: "public, max-age=3600".to_string(),
            base_path: None,
            compression: true,
        }
    }
}

impl EmbeddedFrontend {
    /// Create a new plugin with default settings.
    ///
    /// Defaults: SPA fallback on, `api/` excluded, `assets/` immutable, compression on.
    pub fn new<E: rust_embed::Embed + Send + Sync + 'static>() -> Self {
        Self {
            file_server: Box::new(EmbedAdapter::<E>(std::marker::PhantomData)),
            config: StaticConfig::default(),
        }
    }

    /// Start building a plugin with custom configuration.
    pub fn builder<E: rust_embed::Embed + Send + Sync + 'static>() -> EmbeddedFrontendBuilder {
        EmbeddedFrontendBuilder {
            file_server: Box::new(EmbedAdapter::<E>(std::marker::PhantomData)),
            config: StaticConfig::default(),
        }
    }
}

/// Builder for [`EmbeddedFrontend`] with custom configuration.
pub struct EmbeddedFrontendBuilder {
    file_server: Box<dyn FileServer>,
    config: StaticConfig,
}

impl EmbeddedFrontendBuilder {
    /// Add a path prefix to exclude from static serving (returns 404 immediately).
    ///
    /// By default `api/` is excluded. Call this to add more prefixes.
    /// To clear the default, call [`clear_excluded_prefixes`](Self::clear_excluded_prefixes) first.
    pub fn exclude_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.config.excluded_prefixes.push(prefix.into());
        self
    }

    /// Remove all excluded prefixes (including the default `api/`).
    pub fn clear_excluded_prefixes(mut self) -> Self {
        self.config.excluded_prefixes.clear();
        self
    }

    /// Enable or disable SPA fallback (default: `true`).
    ///
    /// When enabled, requests that don't match any file are served
    /// the `fallback_file` (default `index.html`) instead of 404.
    pub fn spa_fallback(mut self, enabled: bool) -> Self {
        self.config.spa_fallback = enabled;
        self
    }

    /// Set the fallback file for SPA mode (default: `"index.html"`).
    pub fn fallback_file(mut self, file: impl Into<String>) -> Self {
        self.config.fallback_file = file.into();
        self
    }

    /// Set the `Cache-Control` header for SPA fallback responses (default: `"no-cache"`).
    ///
    /// The fallback file (typically `index.html`) should not be cached aggressively
    /// since it references hashed assets that may change on deploy.
    pub fn fallback_cache_control(mut self, value: impl Into<String>) -> Self {
        self.config.fallback_cache_control = value.into();
        self
    }

    /// Set the prefix for files that get immutable cache headers (default: `"assets/"`).
    ///
    /// Pass `None` to disable immutable caching.
    pub fn immutable_prefix(mut self, prefix: impl Into<Option<String>>) -> Self {
        self.config.immutable_prefix = prefix.into();
        self
    }

    /// Set the `Cache-Control` header for immutable files.
    ///
    /// Default: `"public, max-age=31536000, immutable"`.
    pub fn immutable_cache_control(mut self, value: impl Into<String>) -> Self {
        self.config.immutable_cache_control = value.into();
        self
    }

    /// Set the default `Cache-Control` header for non-immutable files.
    ///
    /// Default: `"public, max-age=3600"`.
    pub fn default_cache_control(mut self, value: impl Into<String>) -> Self {
        self.config.default_cache_control = value.into();
        self
    }

    /// Enable or disable pre-compressed file serving (default: `true`).
    ///
    /// When enabled, the server looks for `.br` (Brotli) and `.gz` (gzip) variants
    /// of each requested file and serves them when the client supports the encoding.
    pub fn compression(mut self, enabled: bool) -> Self {
        self.config.compression = enabled;
        self
    }

    /// Mount the static files under a sub-path (e.g. `"/docs"`).
    ///
    /// The base path is stripped before looking up files.
    pub fn base_path(mut self, path: impl Into<String>) -> Self {
        let mut p = path.into();
        if !p.starts_with('/') {
            p.insert(0, '/');
        }
        if p.len() > 1 {
            p = p.trim_end_matches('/').to_string();
        }
        self.config.base_path = Some(p);
        self
    }

    /// Build the [`EmbeddedFrontend`] plugin.
    pub fn build(self) -> EmbeddedFrontend {
        EmbeddedFrontend {
            file_server: self.file_server,
            config: self.config,
        }
    }
}

// ── Static file handler ────────────────────────────────────────────────────

struct StaticFileHandler {
    file_server: Box<dyn FileServer>,
    config: StaticConfig,
}

impl StaticFileHandler {
    fn serve(&self, path: &str, req_headers: &HeaderMap) -> Response {
        let clean = path.trim_start_matches('/');

        for prefix in &self.config.excluded_prefixes {
            if clean.starts_with(prefix.as_str()) {
                return not_found();
            }
        }

        if let Some(resp) = self.try_serve_file(clean, req_headers, None) {
            return resp;
        }

        if clean.is_empty() || clean.ends_with('/') {
            let index_path = format!("{}index.html", clean);
            if let Some(resp) = self.try_serve_file(&index_path, req_headers, None) {
                return resp;
            }
        }

        if self.config.spa_fallback {
            if let Some(resp) = self.try_serve_file(
                &self.config.fallback_file,
                req_headers,
                Some(&self.config.fallback_cache_control),
            ) {
                return resp;
            }
        }

        not_found()
    }

    fn resolve_file(
        &self,
        path: &str,
        req_headers: &HeaderMap,
    ) -> Option<(EmbeddedFileData, Option<&'static str>)> {
        if self.config.compression {
            if let Some(accept) = req_headers.get(ACCEPT_ENCODING) {
                if let Ok(accept_str) = accept.to_str() {
                    if accepts_encoding(accept_str, "br") {
                        let br_path = format!("{}.br", path);
                        if let Some(file) = self.file_server.get_file(&br_path) {
                            return Some((file, Some("br")));
                        }
                    }
                    if accepts_encoding(accept_str, "gzip") {
                        let gz_path = format!("{}.gz", path);
                        if let Some(file) = self.file_server.get_file(&gz_path) {
                            return Some((file, Some("gzip")));
                        }
                    }
                }
            }
        }

        self.file_server.get_file(path).map(|f| (f, None))
    }

    fn try_serve_file(
        &self,
        path: &str,
        req_headers: &HeaderMap,
        cache_override: Option<&str>,
    ) -> Option<Response> {
        let (file, encoding) = self.resolve_file(path, req_headers)?;

        let mime = mime_guess::from_path(path)
            .first_or_octet_stream()
            .to_string();

        let is_immutable = self
            .config
            .immutable_prefix
            .as_ref()
            .is_some_and(|prefix| path.starts_with(prefix.as_str()));

        let cache_control = cache_override.unwrap_or(if is_immutable {
            &self.config.immutable_cache_control
        } else {
            &self.config.default_cache_control
        });

        let etag = file.hash.as_ref().map(|h| format!("\"{}\"", h));

        // 304 Not Modified
        if let Some(ref etag_val) = etag {
            if let Some(inm) = req_headers.get(IF_NONE_MATCH) {
                if let Ok(inm_str) = inm.to_str() {
                    if inm_str == "*" || etag_matches(inm_str, etag_val) {
                        let mut builder = Response::builder()
                            .status(StatusCode::NOT_MODIFIED)
                            .header(ETAG, etag_val.as_str())
                            .header(CACHE_CONTROL, cache_control);
                        if self.config.compression {
                            builder = builder.header(VARY, "Accept-Encoding");
                        }
                        return Some(
                            builder.body(Body::empty()).unwrap().into_response(),
                        );
                    }
                }
            }
        }

        let data_len = file.data.len() as u64;

        // Range requests (skip for compressed responses)
        if encoding.is_none() {
            if let Some(range_hdr) = req_headers.get(RANGE) {
                if let Ok(range_str) = range_hdr.to_str() {
                    return Some(match parse_byte_range(range_str, data_len) {
                        Some(range) => {
                            let start = range.start as usize;
                            let end = range.end_inclusive as usize;
                            let slice = file.data[start..=end].to_vec();
                            let content_range = format!(
                                "bytes {}-{}/{}",
                                range.start, range.end_inclusive, data_len,
                            );
                            let mut builder = Response::builder()
                                .status(StatusCode::PARTIAL_CONTENT)
                                .header(CONTENT_TYPE, &mime)
                                .header(CONTENT_LENGTH, slice.len().to_string())
                                .header(CONTENT_RANGE, content_range)
                                .header(ACCEPT_RANGES, "bytes")
                                .header(CACHE_CONTROL, cache_control);
                            if let Some(ref etag_val) = etag {
                                builder = builder.header(ETAG, etag_val.as_str());
                            }
                            if self.config.compression {
                                builder = builder.header(VARY, "Accept-Encoding");
                            }
                            builder
                                .body(Body::from(slice))
                                .unwrap()
                                .into_response()
                        }
                        None => range_not_satisfiable(data_len),
                    });
                }
            }
        }

        // Full 200 response
        let mut builder = Response::builder()
            .status(StatusCode::OK)
            .header(CONTENT_TYPE, mime)
            .header(CONTENT_LENGTH, data_len.to_string())
            .header(CACHE_CONTROL, cache_control);

        if encoding.is_none() {
            builder = builder.header(ACCEPT_RANGES, "bytes");
        }

        if let Some(ref etag_val) = etag {
            builder = builder.header(ETAG, etag_val.as_str());
        }

        if let Some(enc) = encoding {
            builder = builder.header(CONTENT_ENCODING, enc);
        }

        if self.config.compression {
            builder = builder.header(VARY, "Accept-Encoding");
        }

        Some(
            builder
                .body(Body::from(file.data))
                .unwrap()
                .into_response(),
        )
    }
}

fn not_found() -> Response {
    (StatusCode::NOT_FOUND, "Not Found").into_response()
}

fn range_not_satisfiable(file_size: u64) -> Response {
    Response::builder()
        .status(StatusCode::RANGE_NOT_SATISFIABLE)
        .header(CONTENT_RANGE, format!("bytes */{}", file_size))
        .body(Body::empty())
        .unwrap()
        .into_response()
}

// ── Nestable Router ────────────────────────────────────────────────────────

impl EmbeddedFrontend {
    /// Produce a standalone [`Router`](r2e_core::http::Router) that can be
    /// nested under a sub-path via `Router::nest("/ui", frontend.into_router())`.
    ///
    /// SPA fallback, ETag, cache headers, and MIME detection all work in nested
    /// mode. The `base_path` config is ignored — path stripping is handled by
    /// Axum's `nest`.
    pub fn into_router(self) -> r2e_core::http::Router {
        let handler = Arc::new(StaticFileHandler {
            file_server: self.file_server,
            config: self.config,
        });

        r2e_core::http::Router::new().fallback(
            move |req: r2e_core::http::extract::Request| async move {
                let path = req.uri().path().to_string();
                handler.serve(&path, req.headers())
            },
        )
    }
}

// ── Plugin implementation ──────────────────────────────────────────────────

impl r2e_core::plugin::Plugin for EmbeddedFrontend {
    fn install<T: Clone + Send + Sync + 'static>(
        self,
        app: r2e_core::AppBuilder<T>,
    ) -> r2e_core::AppBuilder<T> {
        let handler = Arc::new(StaticFileHandler {
            file_server: self.file_server,
            config: self.config,
        });

        app.with_layer_fn(move |router| {
            let handler = handler.clone();
            router.fallback(
                move |req: r2e_core::http::extract::Request| async move {
                    let path = req.uri().path().to_string();

                    let effective_path =
                        if let Some(ref base) = handler.config.base_path {
                            match path.strip_prefix(base.as_str()) {
                                Some(rest) => {
                                    if rest.is_empty() || rest.starts_with('/') {
                                        rest
                                    } else {
                                        return not_found();
                                    }
                                }
                                None => return not_found(),
                            }
                        } else {
                            &path
                        };

                    handler.serve(effective_path, req.headers())
                },
            )
        })
    }

    fn should_be_last() -> bool {
        true
    }

    fn name() -> &'static str {
        "EmbeddedFrontend"
    }
}
