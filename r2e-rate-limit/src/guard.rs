use std::sync::Once;

use r2e_core::beans::BeanContext;
use r2e_core::config::R2eConfig;
use r2e_core::decorators::guards::{
    ClientIp, Guard, GuardContext, Identity, PreAuthGuard, PreAuthGuardContext,
};
use r2e_core::type_list::{TCons, TNil};
use r2e_core::DecoratorSpec;

use crate::RateLimitRegistry;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateLimitKeyKind {
    Global,
    User,
    Ip,
}

/// Which source an IP-keyed bucket trusts for the client address.
///
/// See [`ClientIp`] for the trust model: `X-Forwarded-For` is forgeable unless
/// the reverse proxy **overwrites** it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpSource {
    /// Leftmost `X-Forwarded-For` entry when it **parses as an IP address**,
    /// else the transport peer address. The default — correct behind a proxy
    /// that overwrites the header, and no longer degenerates to one shared
    /// bucket without a proxy. A malformed header value is treated as absent,
    /// so it neither suppresses the peer fallback nor becomes a bucket key.
    ForwardedThenPeer,
    /// Transport peer address only. Un-forgeable; use it when the app is
    /// exposed directly (no proxy in front).
    Peer,
}

/// Warn once per process when an IP-keyed bucket cannot resolve a client
/// address — every such request shares the `unknown` bucket, which silently
/// turns "per IP" into "global".
fn warn_unresolved_ip(controller: &str, method: &str) {
    static WARNED: Once = Once::new();
    WARNED.call_once(|| {
        tracing::warn!(
            controller,
            method,
            "rate limit: no client IP available (no usable X-Forwarded-For entry — absent or \
             not an IP address — and no ConnectInfo peer address) — every such request shares the `unknown` \
             bucket, so per-IP limiting degrades to a global limit. Put a proxy in \
             front that sets X-Forwarded-For, or serve with connection info \
             (serve_auto does)."
        );
    });
}

/// Resolve the bucket key fragment for an IP-keyed limit.
///
/// `forwarded` is the **parsed** leftmost `X-Forwarded-For` entry: a malformed
/// header value arrives here as `None`, so it can never suppress the peer
/// fallback nor mint a bucket of its own. Both sources render through
/// [`IpAddr`]'s `Display`, which canonicalizes IPv6 aliases — one address, one
/// bucket.
fn ip_fragment(
    source: IpSource,
    forwarded: Option<std::net::IpAddr>,
    peer: Option<std::net::IpAddr>,
    controller: &str,
    method: &str,
) -> String {
    let resolved = match source {
        IpSource::ForwardedThenPeer => forwarded
            .map(ClientIp::Forwarded)
            .or_else(|| peer.map(ClientIp::Peer)),
        IpSource::Peer => peer.map(ClientIp::Peer),
    };
    match resolved {
        Some(ip) => ip.to_string(),
        None => {
            warn_unresolved_ip(controller, method);
            "unknown".to_string()
        }
    }
}

/// Reject a zero-length window at the construction site.
///
/// A zero window makes the refill rate infinite: the bucket returns to capacity
/// on every call and the limit never applies. Literal budgets are a
/// developer-time mistake, so they panic where they are written.
#[track_caller]
fn assert_window(window_secs: u64) {
    assert!(
        window_secs > 0,
        "rate limit: `window_secs` must be greater than 0 — a zero-second window \
         refills the bucket on every request, which disables the limit entirely"
    );
}

/// Post-authentication rate limit config.
///
/// A plain config value used with `#[guard(...)]`. Its [`DecoratorSpec`] impl
/// pulls the [`RateLimitRegistry`] bean from the graph at controller
/// registration and moves it into the built [`RateLimitGuard`].
///
/// For budgets that come from configuration instead of literals, use
/// [`ConfiguredRateLimit`].
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
    ///
    /// # Panics
    ///
    /// If `window_secs` is 0 (a zero window disables the limit).
    #[track_caller]
    pub fn per_user(max: u64, window_secs: u64) -> RateLimit {
        assert_window(window_secs);
        RateLimit { max, window_secs }
    }
}

impl DecoratorSpec for RateLimit {
    type Product = RateLimitGuard;
    type Deps = TCons<RateLimitRegistry, TNil>;

    /// Per-user buckets are meaningless without an identity: `#[routes]`
    /// rejects at compile time any placement where the identity is statically
    /// always `None`, and the guard 401s at runtime for an `Option<..>`
    /// identity that came back `None`.
    const REQUIRES_IDENTITY: bool = true;

    fn build(self, ctx: &BeanContext) -> RateLimitGuard {
        RateLimitGuard {
            registry: ctx.get::<RateLimitRegistry>(),
            max: self.max,
            window_secs: self.window_secs,
            key: RateLimitKeyKind::User,
            ip_source: IpSource::ForwardedThenPeer,
            enabled: true,
        }
    }
}

/// Pre-authentication rate limit config.
///
/// A plain config value used with `#[pre_guard(...)]`. Its [`DecoratorSpec`]
/// impl pulls the [`RateLimitRegistry`] bean from the graph at controller
/// registration and moves it into the built [`PreAuthRateLimitGuard`].
///
/// For budgets that come from configuration instead of literals, use
/// [`ConfiguredPreRateLimit`].
///
/// # Examples
///
/// ```ignore
/// use r2e::r2e_rate_limit::PreRateLimit;
///
/// #[pre_guard(PreRateLimit::global(5, 60))]  // 5 req / 60 sec, global
/// #[pre_guard(PreRateLimit::per_ip(5, 60))]  // 5 req / 60 sec, per IP
/// #[pre_guard(PreRateLimit::per_ip(5, 60).peer_ip_only())]  // ignore X-Forwarded-For
/// ```
pub struct PreRateLimit {
    max: u64,
    window_secs: u64,
    key: RateLimitKeyKind,
    ip_source: IpSource,
}

impl PreRateLimit {
    /// Global rate limit (shared bucket). Use with `#[pre_guard(...)]`.
    ///
    /// All requests to the annotated handler share the same token bucket
    /// regardless of user or IP (the bucket is still per controller+handler).
    ///
    /// # Panics
    ///
    /// If `window_secs` is 0 (a zero window disables the limit).
    #[track_caller]
    pub fn global(max: u64, window_secs: u64) -> PreRateLimit {
        assert_window(window_secs);
        PreRateLimit {
            max,
            window_secs,
            key: RateLimitKeyKind::Global,
            ip_source: IpSource::ForwardedThenPeer,
        }
    }

    /// Per-IP rate limit. Use with `#[pre_guard(...)]`.
    ///
    /// The client IP is the leftmost `X-Forwarded-For` entry when it parses as
    /// an IP address, otherwise the transport peer address. Use [`peer_ip_only`]
    /// when no proxy sits in front (the header is client-controlled).
    ///
    /// [`peer_ip_only`]: PreRateLimit::peer_ip_only
    ///
    /// # Panics
    ///
    /// If `window_secs` is 0 (a zero window disables the limit).
    #[track_caller]
    pub fn per_ip(max: u64, window_secs: u64) -> PreRateLimit {
        assert_window(window_secs);
        PreRateLimit {
            max,
            window_secs,
            key: RateLimitKeyKind::Ip,
            ip_source: IpSource::ForwardedThenPeer,
        }
    }

    /// Key exclusively on the transport peer address, ignoring
    /// `X-Forwarded-For` (which the client can forge when no trusted proxy
    /// overwrites it).
    pub fn peer_ip_only(mut self) -> PreRateLimit {
        self.ip_source = IpSource::Peer;
        self
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
            ip_source: self.ip_source,
            enabled: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Config-resolved budgets
// ---------------------------------------------------------------------------

/// Read one config key, applying `default` **only when the key is absent**.
///
/// A present-but-invalid value (`max: typo`, `enabled: yes-please`) panics.
/// Specs are built once, at controller registration, so this is the fail-fast
/// point: a security budget must never silently revert to a default because
/// somebody mistyped it.
fn strict_or_default<V>(config: &R2eConfig, key: &str, default: V) -> V
where
    V: r2e_core::config::FromConfigValue + std::fmt::Display,
{
    match config.get_opt::<V>(key) {
        Ok(Some(value)) => value,
        Ok(None) => default,
        Err(err) => panic!(
            "rate limit: invalid configuration for `{key}`: {err}. Fix the value, or remove \
             the key to fall back to the default ({default})."
        ),
    }
}

/// Budget resolved from configuration at controller registration.
///
/// Shared by [`ConfiguredPreRateLimit`] and [`ConfiguredRateLimit`]. Keys are
/// read under the site's prefix:
///
/// | Key | Type | Default |
/// |---|---|---|
/// | `<prefix>.max` | `u64` | the spec's literal default |
/// | `<prefix>.window-secs` | `u64` | the spec's literal default (must be > 0) |
/// | `<prefix>.enabled` | `bool` | `true` |
/// | `<prefix>.trust-forwarded-for` | `bool` | `true` (IP-keyed only) |
///
/// The default applies **only when the key is absent**. A key that is present
/// but not convertible to its type — or a `window-secs` of 0 — aborts startup
/// (panic from `DecoratorSpec::build`), instead of silently reinstating the
/// default budget.
struct ConfiguredBudget {
    prefix: &'static str,
    default_max: u64,
    default_window_secs: u64,
}

impl ConfiguredBudget {
    fn resolve(&self, config: &R2eConfig, ip_source: IpSource) -> (u64, u64, bool, IpSource) {
        let max = strict_or_default(config, &format!("{}.max", self.prefix), self.default_max);
        let window_key = format!("{}.window-secs", self.prefix);
        let window_secs = strict_or_default(config, &window_key, self.default_window_secs);
        assert!(
            window_secs > 0,
            "rate limit: `{window_key}` must be greater than 0 — a zero-second window refills \
             the bucket on every request, which disables the limit entirely"
        );
        let enabled = strict_or_default(config, &format!("{}.enabled", self.prefix), true);
        let trust_forwarded =
            strict_or_default(config, &format!("{}.trust-forwarded-for", self.prefix), true);
        let ip_source = match trust_forwarded {
            true => ip_source,
            false => IpSource::Peer,
        };
        (max, window_secs, enabled, ip_source)
    }
}

/// Pre-authentication rate limit whose budget comes from configuration.
///
/// Same guard as [`PreRateLimit`], but `max` / `window-secs` / `enabled` /
/// `trust-forwarded-for` are read from [`R2eConfig`] at controller
/// registration, so a deployment can retune limits (or switch them off in the
/// `test` profile) without a rebuild. The literal arguments are the defaults
/// used when a key is absent.
///
/// ```yaml
/// rate-limit:
///   public:
///     max: 30
///     window-secs: 60
///     enabled: true
///     trust-forwarded-for: true
/// ```
///
/// ```ignore
/// #[pre_guard(ConfiguredPreRateLimit::per_ip("rate-limit.public").defaults(30, 60))]
/// ```
///
/// This is a **separate spec type** on purpose: it reads `R2eConfig` in
/// addition to `RateLimitRegistry`, so its `DecoratorSpec::Deps` is longer than
/// [`PreRateLimit`]'s and cannot be a constructor on the same type (the
/// attribute's leading type path determines the folded dep list).
pub struct ConfiguredPreRateLimit {
    budget: ConfiguredBudget,
    key: RateLimitKeyKind,
    ip_source: IpSource,
}

impl ConfiguredPreRateLimit {
    /// Global rate limit (shared bucket) with a config-resolved budget.
    pub fn global(prefix: &'static str) -> ConfiguredPreRateLimit {
        ConfiguredPreRateLimit {
            budget: ConfiguredBudget {
                prefix,
                default_max: 60,
                default_window_secs: 60,
            },
            key: RateLimitKeyKind::Global,
            ip_source: IpSource::ForwardedThenPeer,
        }
    }

    /// Per-IP rate limit with a config-resolved budget.
    pub fn per_ip(prefix: &'static str) -> ConfiguredPreRateLimit {
        ConfiguredPreRateLimit {
            budget: ConfiguredBudget {
                prefix,
                default_max: 60,
                default_window_secs: 60,
            },
            key: RateLimitKeyKind::Ip,
            ip_source: IpSource::ForwardedThenPeer,
        }
    }

    /// Defaults used when `<prefix>.max` / `<prefix>.window-secs` are absent.
    ///
    /// # Panics
    ///
    /// If `window_secs` is 0 (a zero window disables the limit).
    #[track_caller]
    pub fn defaults(mut self, max: u64, window_secs: u64) -> ConfiguredPreRateLimit {
        assert_window(window_secs);
        self.budget.default_max = max;
        self.budget.default_window_secs = window_secs;
        self
    }

    /// Ignore `X-Forwarded-For` regardless of configuration (peer address
    /// only). `<prefix>.trust-forwarded-for: false` does the same from config.
    pub fn peer_ip_only(mut self) -> ConfiguredPreRateLimit {
        self.ip_source = IpSource::Peer;
        self
    }
}

impl DecoratorSpec for ConfiguredPreRateLimit {
    type Product = PreAuthRateLimitGuard;
    type Deps = TCons<RateLimitRegistry, TCons<R2eConfig, TNil>>;

    fn build(self, ctx: &BeanContext) -> PreAuthRateLimitGuard {
        let config = ctx.get::<R2eConfig>();
        let (max, window_secs, enabled, ip_source) = self.budget.resolve(&config, self.ip_source);
        PreAuthRateLimitGuard {
            registry: ctx.get::<RateLimitRegistry>(),
            max,
            window_secs,
            key: self.key,
            ip_source,
            enabled,
        }
    }
}

/// Post-authentication rate limit whose budget comes from configuration.
///
/// The [`ConfiguredPreRateLimit`] counterpart for `#[guard(...)]` (per-user).
///
/// ```ignore
/// #[guard(ConfiguredRateLimit::per_user("rate-limit.api").defaults(5, 60))]
/// ```
pub struct ConfiguredRateLimit {
    budget: ConfiguredBudget,
}

impl ConfiguredRateLimit {
    /// Per-user rate limit (requires identity) with a config-resolved budget.
    pub fn per_user(prefix: &'static str) -> ConfiguredRateLimit {
        ConfiguredRateLimit {
            budget: ConfiguredBudget {
                prefix,
                default_max: 60,
                default_window_secs: 60,
            },
        }
    }

    /// Defaults used when `<prefix>.max` / `<prefix>.window-secs` are absent.
    ///
    /// # Panics
    ///
    /// If `window_secs` is 0 (a zero window disables the limit).
    #[track_caller]
    pub fn defaults(mut self, max: u64, window_secs: u64) -> ConfiguredRateLimit {
        assert_window(window_secs);
        self.budget.default_max = max;
        self.budget.default_window_secs = window_secs;
        self
    }
}

impl DecoratorSpec for ConfiguredRateLimit {
    type Product = RateLimitGuard;
    type Deps = TCons<RateLimitRegistry, TCons<R2eConfig, TNil>>;

    /// Per-user buckets require an identity — same contract as [`RateLimit`].
    const REQUIRES_IDENTITY: bool = true;

    fn build(self, ctx: &BeanContext) -> RateLimitGuard {
        let config = ctx.get::<R2eConfig>();
        let (max, window_secs, enabled, ip_source) =
            self.budget.resolve(&config, IpSource::ForwardedThenPeer);
        RateLimitGuard {
            registry: ctx.get::<RateLimitRegistry>(),
            max,
            window_secs,
            key: RateLimitKeyKind::User,
            ip_source,
            enabled,
        }
    }
}

// ---------------------------------------------------------------------------
// Guards
// ---------------------------------------------------------------------------

fn too_many_requests() -> r2e_core::http::Response {
    r2e_core::http::response::static_json(
        r2e_core::http::StatusCode::TOO_MANY_REQUESTS,
        r#"{"error":"Rate limit exceeded"}"#,
    )
}

/// A per-user limit reached without an identity: fail closed.
///
/// Sharing an `anonymous` bucket would turn a per-user budget into a global one
/// for every unauthenticated caller — the opposite of what the annotation
/// promises. [`DecoratorSpec::REQUIRES_IDENTITY`] rejects statically
/// identity-less placements at compile time; this is the runtime backstop for an
/// `Option<..>` identity that came back `None`.
fn identity_required(controller: &str, method: &str) -> r2e_core::http::Response {
    tracing::warn!(
        controller,
        method,
        "rate limit: per-user limit on a request with no identity — rejecting with 401 \
         (a per-user bucket cannot be keyed without a subject)"
    );
    r2e_core::http::response::static_json(
        r2e_core::http::StatusCode::UNAUTHORIZED,
        r#"{"error":"Authentication required for this rate-limited endpoint"}"#,
    )
}

/// Post-authentication rate limit guard.
///
/// Holds the [`RateLimitRegistry`] as a field (resolved once at controller
/// registration via [`RateLimit`]'s [`DecoratorSpec`] impl) — there is no
/// state lookup at request time.
///
/// Bucket keys are prefixed with the controller name **and** the handler name,
/// so two controllers that expose homonymous handlers (`start`, `answer`, …)
/// never share a bucket.
pub struct RateLimitGuard {
    pub registry: RateLimitRegistry,
    pub max: u64,
    pub window_secs: u64,
    pub key: RateLimitKeyKind,
    pub ip_source: IpSource,
    pub enabled: bool,
}

impl<I: Identity> Guard<I> for RateLimitGuard {
    fn check(
        &self,
        ctx: &GuardContext<'_, I>,
    ) -> impl std::future::Future<Output = Result<(), r2e_core::http::Response>> + Send {
        if !self.enabled {
            return std::future::ready(Ok(()));
        }
        let scope = format!("{}:{}", ctx.controller_name, ctx.method_name);
        let key = match self.key {
            RateLimitKeyKind::Global => format!("{scope}:global"),
            RateLimitKeyKind::User => {
                let Some(sub) = ctx.identity.map(|i| i.sub()) else {
                    return std::future::ready(Err(identity_required(
                        ctx.controller_name,
                        ctx.method_name,
                    )));
                };
                format!("{scope}:user:{sub}")
            }
            RateLimitKeyKind::Ip => {
                let ip = ip_fragment(
                    self.ip_source,
                    ctx.forwarded_ip(),
                    ctx.peer_ip(),
                    ctx.controller_name,
                    ctx.method_name,
                );
                format!("{scope}:ip:{ip}")
            }
        };
        let result = if self.registry.try_acquire(&key, self.max, self.window_secs) {
            Ok(())
        } else {
            Err(too_many_requests())
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
///
/// Bucket keys are prefixed with the controller name **and** the handler name,
/// so two controllers that expose homonymous handlers (`start`, `answer`, …)
/// never share a bucket.
pub struct PreAuthRateLimitGuard {
    pub registry: RateLimitRegistry,
    pub max: u64,
    pub window_secs: u64,
    pub key: RateLimitKeyKind,
    pub ip_source: IpSource,
    pub enabled: bool,
}

impl PreAuthGuard for PreAuthRateLimitGuard {
    fn check(
        &self,
        ctx: &PreAuthGuardContext<'_>,
    ) -> impl std::future::Future<Output = Result<(), r2e_core::http::Response>> + Send {
        if !self.enabled {
            return std::future::ready(Ok(()));
        }
        let scope = format!("{}:{}", ctx.controller_name, ctx.method_name);
        let key = match self.key {
            RateLimitKeyKind::Global => format!("{scope}:global"),
            RateLimitKeyKind::Ip => {
                let ip = ip_fragment(
                    self.ip_source,
                    ctx.forwarded_ip(),
                    ctx.peer_ip(),
                    ctx.controller_name,
                    ctx.method_name,
                );
                format!("{scope}:ip:{ip}")
            }
            RateLimitKeyKind::User => {
                // No public constructor produces this (per-user is `RateLimit` /
                // `ConfiguredRateLimit`, which are post-auth guards); a
                // hand-built `PreAuthRateLimitGuard` with `User` has no identity
                // to key on, so tighten to the global bucket rather than invent
                // an `anonymous` one.
                format!("{scope}:global")
            }
        };
        let result = if self.registry.try_acquire(&key, self.max, self.window_secs) {
            Ok(())
        } else {
            Err(too_many_requests())
        };
        std::future::ready(result)
    }
}
