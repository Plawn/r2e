//! Embedded static file serving with SPA support for R2E.
//!
//! This crate provides an [`EmbeddedFrontend`] plugin that serves static files
//! embedded in the binary via [`rust_embed`], with optional SPA fallback support.
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
//!     .build())
//! ```

use std::borrow::Cow;
use std::sync::Arc;

use r2e_core::http::response::IntoResponse;
pub use rust_embed;

// ── File server abstraction ─────────────────────────────────────────────────

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
        let hash = {
            let h = file.metadata.sha256_hash();
            // Convert the fixed-size hash to a hex string for ETag.
            let hex: String = h.iter().map(|b| format!("{:02x}", b)).collect();
            Some(hex)
        };
        Some(EmbeddedFileData {
            data: file.data,
            hash,
        })
    }
}

// ── Configuration & Builder ─────────────────────────────────────────────────

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
    immutable_prefix: Option<String>,
    immutable_cache_control: String,
    default_cache_control: String,
    base_path: Option<String>,
}

impl Default for StaticConfig {
    fn default() -> Self {
        Self {
            excluded_prefixes: vec!["api/".to_string()],
            spa_fallback: true,
            fallback_file: "index.html".to_string(),
            immutable_prefix: Some("assets/".to_string()),
            immutable_cache_control: "public, max-age=31536000, immutable".to_string(),
            default_cache_control: "public, max-age=3600".to_string(),
            base_path: None,
        }
    }
}

impl EmbeddedFrontend {
    /// Create a new plugin with default settings.
    ///
    /// Defaults: SPA fallback on, `api/` excluded, `assets/` immutable.
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

    /// Mount the static files under a sub-path (e.g. `"/docs"`).
    ///
    /// The base path is stripped before looking up files.
    pub fn base_path(mut self, path: impl Into<String>) -> Self {
        let mut p = path.into();
        // Normalize: ensure leading slash, no trailing slash.
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

// ── Static file handler ─────────────────────────────────────────────────────

struct StaticFileHandler {
    file_server: Box<dyn FileServer>,
    config: StaticConfig,
}

impl StaticFileHandler {
    fn serve(&self, path: &str) -> r2e_core::http::Response {
        let clean = path.trim_start_matches('/');

        // 1. Check excluded prefixes.
        for prefix in &self.config.excluded_prefixes {
            if clean.starts_with(prefix.as_str()) {
                return not_found();
            }
        }

        // 2. Exact file match.
        if let Some(resp) = self.try_serve_file(clean) {
            return resp;
        }

        // 3. Directory index: foo/ → foo/index.html
        if clean.is_empty() || clean.ends_with('/') {
            let index_path = format!("{}index.html", clean);
            if let Some(resp) = self.try_serve_file(&index_path) {
                return resp;
            }
        }

        // 4. SPA fallback.
        if self.config.spa_fallback {
            if let Some(resp) = self.try_serve_file(&self.config.fallback_file) {
                return resp;
            }
        }

        // 5. 404.
        not_found()
    }

    fn try_serve_file(&self, path: &str) -> Option<r2e_core::http::Response> {
        let file = self.file_server.get_file(path)?;

        let mime = mime_guess::from_path(path)
            .first_or_octet_stream()
            .to_string();

        let is_immutable = self
            .config
            .immutable_prefix
            .as_ref()
            .is_some_and(|prefix| path.starts_with(prefix.as_str()));

        let cache_control = if is_immutable {
            &self.config.immutable_cache_control
        } else {
            &self.config.default_cache_control
        };

        let mut builder = r2e_core::http::Response::builder()
            .status(r2e_core::http::StatusCode::OK)
            .header(r2e_core::http::header::CONTENT_TYPE, mime)
            .header(r2e_core::http::header::CACHE_CONTROL, cache_control);

        if let Some(hash) = &file.hash {
            builder = builder.header(r2e_core::http::header::ETAG, format!("\"{}\"", hash));
        }

        Some(
            builder
                .body(r2e_core::http::Body::from(file.data))
                .unwrap()
                .into_response(),
        )
    }
}

fn not_found() -> r2e_core::http::Response {
    (r2e_core::http::StatusCode::NOT_FOUND, "Not Found").into_response()
}

// ── Plugin implementation ───────────────────────────────────────────────────

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
            router.fallback(move |req: r2e_core::http::Request| async move {
                let path = req.uri().path().to_string();

                // Strip base_path prefix if configured.
                let effective_path = if let Some(ref base) = handler.config.base_path {
                    match path.strip_prefix(base.as_str()) {
                        Some(rest) => {
                            if rest.is_empty() || rest.starts_with('/') {
                                rest
                            } else {
                                // Path doesn't match base (e.g. base=/docs, path=/documentation).
                                return not_found();
                            }
                        }
                        None => return not_found(),
                    }
                } else {
                    &path
                };

                handler.serve(effective_path)
            })
        })
    }

    fn should_be_last() -> bool {
        true
    }

    fn name() -> &'static str {
        "EmbeddedFrontend"
    }
}
