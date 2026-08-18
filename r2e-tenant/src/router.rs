//! [`TenantRouter`] — the "tenancy is installed" witness bean.
//!
//! One bean, one `TypeId`, no generics: every per-tenant extractor and every
//! `#[managed]` per-tenant resource requires it in the state, which is what
//! turns "you forgot `.plugin(Tenancy::resolver::<_>())`" into a **compile**
//! error at `register_controllers` instead of a 500 on the first request.
//!
//! It owns the resolver, the missing-tenant policy, and the configured
//! statuses — and the per-request memo: a framework-owned resolve-once cell
//! parked in `parts.extensions` by the [`Tenancy`](crate::Tenancy) layer
//! *before routing*, so the extractor, the guards and every `#[managed]`
//! resource of one request share at most one *successful* resolver call. An
//! error is not memoized: it goes back to that caller, and the next resolution
//! attempt in the request tries again.

use std::sync::Arc;

use r2e_core::http::extract::FromRequestParts;
use r2e_core::http::{Extensions, Parts};
use r2e_core::web::request_head::RequestHead;
use r2e_core::HttpError;

use crate::config::{MissingTenantPolicy, TenancyConfig};
use crate::error::{TenantError, TenantStatuses};
use crate::resolver::TenantResolver;
use crate::TenantId;

/// Resolves the tenant of a request, according to the deployment's policy.
///
/// Built whole by the [`Tenancy`](crate::Tenancy) plugin's `build` — either
/// [`ready`](Self::ready) (resolver wired) or [`disabled`](Self::disabled)
/// (`tenancy.enabled: false`). There is no unwired state.
#[derive(Clone)]
pub struct TenantRouter {
    mode: Arc<Mode>,
}

/// The per-request resolve-once cell.
///
/// **Private on purpose.** The memo used to be a bare [`TenantId`] extension,
/// which made any `TenantId` a middleware happened to park in the request
/// authoritative over the configured resolver. The carrier is now a type no
/// application can name, let alone insert: the only way into the cell is
/// through [`TenantRouter`].
///
/// It holds the *resolver's own answer* (`Option<TenantId>`, i.e. before the
/// missing-tenant policy is applied), so a `None` — "this request carries no
/// tenant" — is memoized just like a hit and the policy stays a per-call-site
/// decision. Resolver **errors** are deliberately not memoized: the cell is
/// left empty (`get_or_try_init` semantics) because a failing request is about
/// to end anyway.
#[derive(Clone, Default)]
struct TenantMemo(Arc<tokio::sync::OnceCell<Option<TenantId>>>);

impl TenantMemo {
    /// The memoized answer, if resolution already happened in this request.
    fn peek(&self) -> Option<&TenantId> {
        self.0.get().and_then(Option::as_ref)
    }

    /// Run `resolve` at most once per request **on the success path**, sharing
    /// its answer (and, for concurrent callers, the single in-flight call).
    ///
    /// An `Err` is returned to that caller without being memoized — `OnceCell`
    /// leaves the cell empty — so a later component in the same request resolves
    /// again. Only a successful answer (including `Ok(None)`, "no tenant on this
    /// request") is remembered.
    async fn resolve_once<F, Fut>(&self, resolve: F) -> Result<Option<TenantId>, HttpError>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<Option<TenantId>, HttpError>>,
    {
        self.0.get_or_try_init(resolve).await.cloned()
    }
}

enum Mode {
    /// `tenancy.enabled = false`: the app boots, nothing resolves.
    Disabled { statuses: TenantStatuses },
    Ready {
        resolver: Arc<dyn TenantResolver>,
        policy: MissingTenantPolicy,
        statuses: TenantStatuses,
    },
}

impl TenantRouter {
    /// A router that resolves nothing (`tenancy.enabled = false`).
    #[must_use]
    pub fn disabled(statuses: TenantStatuses) -> Self {
        Self {
            mode: Arc::new(Mode::Disabled { statuses }),
        }
    }

    /// A wired router. Used by the plugin's `build`; also the entry point
    /// for tests that build a router without the builder.
    ///
    /// Providing one of these instead of installing
    /// [`Tenancy`](crate::Tenancy) skips the layer that parks the per-request
    /// resolve-once cell, so resolution is memoized only from the point an
    /// extractor runs. Add [`install_memo`](Self::install_memo) to a middleware
    /// of your own to get the whole-request guarantee back.
    pub fn ready(
        resolver: Arc<dyn TenantResolver>,
        policy: MissingTenantPolicy,
        statuses: TenantStatuses,
    ) -> Self {
        Self {
            mode: Arc::new(Mode::Ready {
                resolver,
                policy,
                statuses,
            }),
        }
    }

    /// A wired router with policy and statuses read from `config`.
    pub(crate) fn from_config(resolver: Arc<dyn TenantResolver>, config: &TenancyConfig) -> Self {
        Self::ready(resolver, config.policy(), config.statuses())
    }

    /// The configured statuses.
    #[must_use]
    pub fn statuses(&self) -> TenantStatuses {
        match &*self.mode {
            Mode::Ready { statuses, .. } | Mode::Disabled { statuses } => *statuses,
        }
    }

    /// The missing-tenant policy. A disabled router reports
    /// [`MissingTenantPolicy::Allow`]: `Option` extractors yield `None` rather
    /// than failing a whole app that deliberately turned tenancy off.
    #[must_use]
    pub fn policy(&self) -> MissingTenantPolicy {
        match &*self.mode {
            Mode::Ready { policy, .. } => *policy,
            Mode::Disabled { .. } => MissingTenantPolicy::Allow,
        }
    }

    /// Whether a resolver is wired (`tenancy.enabled != false`).
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        matches!(&*self.mode, Mode::Ready { .. })
    }

    /// Install the per-request resolve-once cell into `extensions`.
    ///
    /// The [`Tenancy`](crate::Tenancy) plugin installs a router layer that calls
    /// this before routing, which is what makes "resolve at most once per
    /// request" true (on the success path — an error is not memoized and the
    /// next attempt retries) for **every** consumer — including a handler whose
    /// only tenancy is two
    /// `#[managed]` resources and which therefore never runs a tenancy
    /// extractor. Extensions are cloned by value into the request head the
    /// generated handlers build, but the cell itself is an `Arc`, so every copy
    /// of the extensions shares one answer.
    ///
    /// Public for hand-wired routers that provide a [`TenantRouter`] directly
    /// instead of installing [`Tenancy`](crate::Tenancy) — without the cell,
    /// each guard / extractor / `#[managed]` resource resolves for itself.
    /// Installing it twice is a no-op, and installing it is all an outsider can
    /// do: there is no public way to *fill* it, so the configured resolver stays
    /// the only authority on which tenant a request belongs to.
    pub fn install_memo(extensions: &mut Extensions) {
        if extensions.get::<TenantMemo>().is_none() {
            extensions.insert(TenantMemo::default());
        }
    }

    /// The tenant already resolved for this request, if any.
    ///
    /// A read-only peek at the resolve-once cell — `None` both when nothing
    /// resolved yet and when the resolver answered "no tenant". Prefer
    /// [`try_resolve`](Self::try_resolve), which reuses the same cell and
    /// resolves when it is still empty.
    #[must_use]
    pub fn memoized<'a>(head: &RequestHead<'a>) -> Option<&'a TenantId> {
        head.extension::<TenantMemo>().and_then(TenantMemo::peek)
    }

    /// The resolver's own answer for this request, memoized.
    ///
    /// Everything tenancy-related funnels through here, so the resolver runs at
    /// most once per request on the success path, whatever the mix of guards,
    /// extractors and `#[managed]` resources on the route; an **error** is
    /// returned to that caller without being memoized, so the next attempt in
    /// the same request resolves again. Without the cell in the extensions
    /// (a hand-built head in a test, a hand-rolled router) it degrades to
    /// resolving per call.
    async fn resolve_raw(&self, head: &RequestHead<'_>) -> Result<Option<TenantId>, HttpError> {
        match &*self.mode {
            Mode::Disabled { .. } => Ok(None),
            Mode::Ready { resolver, .. } => match head.extension::<TenantMemo>() {
                Some(memo) => memo.resolve_once(|| resolver.resolve(head)).await,
                None => resolver.resolve(head).await,
            },
        }
    }

    /// Resolve the tenant, or `None` when the request carries none and the
    /// policy allows it.
    ///
    /// Shares the request's resolve-once cell with every other caller; the
    /// missing-tenant policy is applied here, on the memoized raw answer.
    pub async fn try_resolve(
        &self,
        head: &RequestHead<'_>,
    ) -> Result<Option<TenantId>, HttpError> {
        match self.resolve_raw(head).await? {
            Some(tenant) => Ok(Some(tenant)),
            None => match self.policy() {
                MissingTenantPolicy::Allow => Ok(None),
                MissingTenantPolicy::Reject => {
                    Err(TenantError::Unresolved.into_http_error(self.statuses()))
                }
            },
        }
    }

    /// Resolve the tenant, failing with `missing-status` when the request
    /// carries none.
    pub async fn resolve(&self, head: &RequestHead<'_>) -> Result<TenantId, HttpError> {
        match self.try_resolve(head).await? {
            Some(tenant) => Ok(tenant),
            None => Err(TenantError::Unresolved.into_http_error(self.statuses())),
        }
    }

    /// Resolve from request parts, through the request's resolve-once cell.
    ///
    /// This is the entry point extractors use. It installs the cell when it is
    /// absent (a hand-built router, a test driving parts directly), so an
    /// extractor-first request memoizes even without the
    /// [`Tenancy`](crate::Tenancy) layer.
    pub async fn resolve_parts<S: Send + Sync>(
        &self,
        parts: &mut Parts,
        state: &S,
    ) -> Result<TenantId, HttpError> {
        match self.try_resolve_parts(parts, state).await? {
            Some(tenant) => Ok(tenant),
            None => Err(TenantError::Unresolved.into_http_error(self.statuses())),
        }
    }

    /// [`try_resolve`](Self::try_resolve) from request parts, through the
    /// request's resolve-once cell (installing it when absent).
    pub async fn try_resolve_parts<S: Send + Sync>(
        &self,
        parts: &mut Parts,
        state: &S,
    ) -> Result<Option<TenantId>, HttpError> {
        Self::install_memo(&mut parts.extensions);
        // `RawPathParams` is cloned out of the extensions axum's router filled
        // in, so a `PathTenantResolver` works from an extractor exactly as it
        // does from a guard.
        let raw = r2e_core::http::extract::RawPathParams::from_request_parts(parts, state)
            .await
            .ok();
        let head = RequestHead {
            method: &parts.method,
            uri: &parts.uri,
            headers: &parts.headers,
            extensions: &parts.extensions,
            path_params: raw
                .as_ref()
                .map_or(r2e_core::PathParams::EMPTY, r2e_core::PathParams::from_raw),
            peer_addr: parts
                .extensions
                .get::<r2e_core::http::ConnectInfo<std::net::SocketAddr>>()
                .map(|info| info.0),
        };
        self.try_resolve(&head).await
    }
}

impl std::fmt::Debug for TenantRouter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = match &*self.mode {
            Mode::Disabled { .. } => "disabled",
            Mode::Ready { .. } => "ready",
        };
        f.debug_struct("TenantRouter").field("state", &state).finish()
    }
}
