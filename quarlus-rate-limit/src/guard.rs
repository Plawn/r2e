use quarlus_core::guards::{Guard, GuardContext, Identity, PreAuthGuard, PreAuthGuardContext};
use quarlus_core::http::extract::FromRef;
use quarlus_core::http::response::IntoResponse;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateLimitKeyKind {
    Global,
    User,
    Ip,
}

/// Builder for rate limit guards.
///
/// Provides static methods to create rate limit guards for use with
/// `#[guard(...)]` or `#[pre_guard(...)]` attributes.
///
/// # Examples
///
/// ```ignore
/// use quarlus::quarlus_rate_limit::RateLimit;
///
/// #[pre_guard(RateLimit::global(5, 60))]    // 5 req / 60 sec, global
/// #[pre_guard(RateLimit::per_ip(5, 60))]    // 5 req / 60 sec, per IP
/// #[guard(RateLimit::per_user(5, 60))]      // 5 req / 60 sec, per user
/// ```
pub struct RateLimit;

impl RateLimit {
    /// Global rate limit (shared bucket). Use with `#[pre_guard(...)]`.
    ///
    /// All requests share the same token bucket regardless of user or IP.
    pub fn global(max: u64, window_secs: u64) -> PreAuthRateLimitGuard {
        PreAuthRateLimitGuard {
            max,
            window_secs,
            key: RateLimitKeyKind::Global,
        }
    }

    /// Per-IP rate limit. Use with `#[pre_guard(...)]`.
    ///
    /// Each unique IP address (from X-Forwarded-For header) gets its own bucket.
    pub fn per_ip(max: u64, window_secs: u64) -> PreAuthRateLimitGuard {
        PreAuthRateLimitGuard {
            max,
            window_secs,
            key: RateLimitKeyKind::Ip,
        }
    }

    /// Per-user rate limit (requires identity). Use with `#[guard(...)]`.
    ///
    /// Each authenticated user (by subject ID) gets their own bucket.
    /// This guard runs after JWT validation.
    pub fn per_user(max: u64, window_secs: u64) -> RateLimitGuard {
        RateLimitGuard {
            max,
            window_secs,
            key: RateLimitKeyKind::User,
        }
    }
}

pub struct RateLimitGuard {
    pub max: u64,
    pub window_secs: u64,
    pub key: RateLimitKeyKind,
}

impl<S: Send + Sync, I: Identity> Guard<S, I> for RateLimitGuard
where
    crate::RateLimitRegistry: FromRef<S>,
{
    fn check(
        &self,
        state: &S,
        ctx: &GuardContext<'_, I>,
    ) -> impl std::future::Future<Output = Result<(), quarlus_core::http::Response>> + Send {
        let result = {
            let registry = <crate::RateLimitRegistry as FromRef<S>>::from_ref(state);
            let method = ctx.method_name;
            let key = match self.key {
                RateLimitKeyKind::Global => format!("{}:global", method),
                RateLimitKeyKind::User => {
                    let sub = ctx
                        .identity
                        .map(|i| i.sub())
                        .unwrap_or("anonymous");
                    format!("{}:user:{}", method, sub)
                }
                RateLimitKeyKind::Ip => {
                    let ip = ctx
                        .headers
                        .get("x-forwarded-for")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|v| v.split(',').next())
                        .map(|s| s.trim())
                        .unwrap_or("unknown");
                    format!("{}:ip:{}", method, ip)
                }
            };
            if registry.try_acquire(&key, self.max, self.window_secs) {
                Ok(())
            } else {
                Err((
                    quarlus_core::http::StatusCode::TOO_MANY_REQUESTS,
                    quarlus_core::http::Json(serde_json::json!({ "error": "Rate limit exceeded" })),
                )
                    .into_response())
            }
        };
        std::future::ready(result)
    }
}

/// Pre-authentication rate limit guard for global and IP-based rate limiting.
///
/// Runs as middleware before JWT extraction, avoiding unnecessary token
/// validation when the request is already rate-limited.
pub struct PreAuthRateLimitGuard {
    pub max: u64,
    pub window_secs: u64,
    pub key: RateLimitKeyKind,
}

impl<S: Send + Sync> PreAuthGuard<S> for PreAuthRateLimitGuard
where
    crate::RateLimitRegistry: FromRef<S>,
{
    fn check(
        &self,
        state: &S,
        ctx: &PreAuthGuardContext<'_>,
    ) -> impl std::future::Future<Output = Result<(), quarlus_core::http::Response>> + Send {
        let result = {
            let registry = <crate::RateLimitRegistry as FromRef<S>>::from_ref(state);
            let method = ctx.method_name;
            let key = match self.key {
                RateLimitKeyKind::Global => format!("{}:global", method),
                RateLimitKeyKind::Ip => {
                    let ip = ctx
                        .headers
                        .get("x-forwarded-for")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|v| v.split(',').next())
                        .map(|s| s.trim())
                        .unwrap_or("unknown");
                    format!("{}:ip:{}", method, ip)
                }
                RateLimitKeyKind::User => {
                    // User-keyed rate limiting should not use PreAuthRateLimitGuard;
                    // fall back to global key as a safety net.
                    format!("{}:global", method)
                }
            };
            if registry.try_acquire(&key, self.max, self.window_secs) {
                Ok(())
            } else {
                Err((
                    quarlus_core::http::StatusCode::TOO_MANY_REQUESTS,
                    quarlus_core::http::Json(serde_json::json!({ "error": "Rate limit exceeded" })),
                )
                    .into_response())
            }
        };
        std::future::ready(result)
    }
}
