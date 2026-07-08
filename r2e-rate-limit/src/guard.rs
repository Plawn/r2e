use r2e_core::beans::BeanContext;
use r2e_core::guards::{Guard, GuardContext, Identity, PreAuthGuard, PreAuthGuardContext};
use r2e_core::http::response::IntoResponse;
use r2e_core::type_list::{TCons, TNil};
use r2e_core::DecoratorSpec;

use crate::RateLimitRegistry;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateLimitKeyKind {
    Global,
    User,
    Ip,
}

/// Post-authentication rate limit config.
///
/// A plain config value used with `#[guard(...)]`. Its [`DecoratorSpec`] impl
/// pulls the [`RateLimitRegistry`] bean from the graph at controller
/// registration and moves it into the built [`RateLimitGuard`].
///
/// # Examples
///
/// ```ignore
/// use r2e::r2e_rate_limit::RateLimit;
///
/// #[guard(RateLimit::per_user(5, 60))]      // 5 req / 60 sec, per user
/// ```
pub struct RateLimit {
    max: u64,
    window_secs: u64,
}

impl RateLimit {
    /// Per-user rate limit (requires identity). Use with `#[guard(...)]`.
    ///
    /// Each authenticated user (by subject ID) gets their own bucket.
    /// This guard runs after JWT validation.
    pub fn per_user(max: u64, window_secs: u64) -> RateLimit {
        RateLimit { max, window_secs }
    }
}

impl DecoratorSpec for RateLimit {
    type Product = RateLimitGuard;
    type Deps = TCons<RateLimitRegistry, TNil>;

    fn build(self, ctx: &BeanContext) -> RateLimitGuard {
        RateLimitGuard {
            registry: ctx.get::<RateLimitRegistry>(),
            max: self.max,
            window_secs: self.window_secs,
            key: RateLimitKeyKind::User,
        }
    }
}

/// Pre-authentication rate limit config.
///
/// A plain config value used with `#[pre_guard(...)]`. Its [`DecoratorSpec`]
/// impl pulls the [`RateLimitRegistry`] bean from the graph at controller
/// registration and moves it into the built [`PreAuthRateLimitGuard`].
///
/// # Examples
///
/// ```ignore
/// use r2e::r2e_rate_limit::PreRateLimit;
///
/// #[pre_guard(PreRateLimit::global(5, 60))]  // 5 req / 60 sec, global
/// #[pre_guard(PreRateLimit::per_ip(5, 60))]  // 5 req / 60 sec, per IP
/// ```
pub struct PreRateLimit {
    max: u64,
    window_secs: u64,
    key: RateLimitKeyKind,
}

impl PreRateLimit {
    /// Global rate limit (shared bucket). Use with `#[pre_guard(...)]`.
    ///
    /// All requests share the same token bucket regardless of user or IP.
    pub fn global(max: u64, window_secs: u64) -> PreRateLimit {
        PreRateLimit {
            max,
            window_secs,
            key: RateLimitKeyKind::Global,
        }
    }

    /// Per-IP rate limit. Use with `#[pre_guard(...)]`.
    ///
    /// Each unique IP address (from X-Forwarded-For header) gets its own bucket.
    pub fn per_ip(max: u64, window_secs: u64) -> PreRateLimit {
        PreRateLimit {
            max,
            window_secs,
            key: RateLimitKeyKind::Ip,
        }
    }
}

impl DecoratorSpec for PreRateLimit {
    type Product = PreAuthRateLimitGuard;
    type Deps = TCons<RateLimitRegistry, TNil>;

    fn build(self, ctx: &BeanContext) -> PreAuthRateLimitGuard {
        PreAuthRateLimitGuard {
            registry: ctx.get::<RateLimitRegistry>(),
            max: self.max,
            window_secs: self.window_secs,
            key: self.key,
        }
    }
}

/// Post-authentication rate limit guard.
///
/// Holds the [`RateLimitRegistry`] as a field (resolved once at controller
/// registration via [`RateLimit`]'s [`DecoratorSpec`] impl) — there is no
/// state lookup at request time.
pub struct RateLimitGuard {
    pub registry: RateLimitRegistry,
    pub max: u64,
    pub window_secs: u64,
    pub key: RateLimitKeyKind,
}

impl<I: Identity> Guard<I> for RateLimitGuard {
    fn check(
        &self,
        ctx: &GuardContext<'_, I>,
    ) -> impl std::future::Future<Output = Result<(), r2e_core::http::Response>> + Send {
        let method = ctx.method_name;
        let key = match self.key {
            RateLimitKeyKind::Global => format!("{}:global", method),
            RateLimitKeyKind::User => {
                let sub = ctx.identity.map(|i| i.sub()).unwrap_or("anonymous");
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
        let result = if self.registry.try_acquire(&key, self.max, self.window_secs) {
            Ok(())
        } else {
            Err((
                r2e_core::http::StatusCode::TOO_MANY_REQUESTS,
                r2e_core::http::Json(serde_json::json!({ "error": "Rate limit exceeded" })),
            )
                .into_response())
        };
        std::future::ready(result)
    }
}

/// Pre-authentication rate limit guard for global and IP-based rate limiting.
///
/// Runs as middleware before JWT extraction, avoiding unnecessary token
/// validation when the request is already rate-limited. Holds the
/// [`RateLimitRegistry`] as a field (resolved once at controller registration
/// via [`PreRateLimit`]'s [`DecoratorSpec`] impl).
pub struct PreAuthRateLimitGuard {
    pub registry: RateLimitRegistry,
    pub max: u64,
    pub window_secs: u64,
    pub key: RateLimitKeyKind,
}

impl PreAuthGuard for PreAuthRateLimitGuard {
    fn check(
        &self,
        ctx: &PreAuthGuardContext<'_>,
    ) -> impl std::future::Future<Output = Result<(), r2e_core::http::Response>> + Send {
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
        let result = if self.registry.try_acquire(&key, self.max, self.window_secs) {
            Ok(())
        } else {
            Err((
                r2e_core::http::StatusCode::TOO_MANY_REQUESTS,
                r2e_core::http::Json(serde_json::json!({ "error": "Rate limit exceeded" })),
            )
                .into_response())
        };
        std::future::ready(result)
    }
}
