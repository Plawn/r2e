//! [`TenantRouter`] — the "tenancy is installed" witness bean.
//!
//! One bean, one `TypeId`, no generics: every per-tenant extractor and every
//! `#[managed]` per-tenant resource requires it in the state, which is what
//! turns "you forgot `.plugin(Tenancy::resolver::<_>())`" into a **compile**
//! error at `register_controllers` instead of a 500 on the first request.
//!
//! It owns the resolver, the missing-tenant policy, and the configured
//! statuses — and the per-request memo: the first component to resolve the
//! tenant parks it in `parts.extensions`, so the extractor, the guards and the
//! managed resources of one request resolve at most once.

use std::sync::Arc;

use r2e_core::http::extract::FromRequestParts;
use r2e_core::http::Parts;
use r2e_core::request_head::RequestHead;
use r2e_core::{HttpError, Late};

use crate::config::{MissingTenantPolicy, TenancyConfig};
use crate::error::{TenantError, TenantStatuses};
use crate::resolver::TenantResolver;
use crate::TenantId;

/// Resolves the tenant of a request, according to the deployment's policy.
///
/// Provided by the [`Tenancy`](crate::Tenancy) plugin as a [`Late`] shell and
/// filled in its `configure` phase.
#[derive(Clone)]
pub struct TenantRouter {
    mode: Late<Mode>,
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
    /// An unwired router — what [`Tenancy::install`](crate::Tenancy) provides
    /// before the resolver bean exists. Resolving through it is a 500 naming the
    /// missing plugin wiring.
    #[must_use]
    pub fn unwired() -> Self {
        Self { mode: Late::new() }
    }

    /// A router that resolves nothing (`tenancy.enabled = false`).
    #[must_use]
    pub fn disabled(statuses: TenantStatuses) -> Self {
        let router = Self::unwired();
        let _ = router.mode.fill(Mode::Disabled { statuses });
        router
    }

    /// A wired router. Used by the plugin's `configure`; also the entry point
    /// for tests that build a router without the builder.
    pub fn ready(
        resolver: Arc<dyn TenantResolver>,
        policy: MissingTenantPolicy,
        statuses: TenantStatuses,
    ) -> Self {
        let router = Self::unwired();
        let _ = router.mode.fill(Mode::Ready {
            resolver,
            policy,
            statuses,
        });
        router
    }

    /// Fill an unwired shell. Returns `false` if it was already filled.
    pub(crate) fn wire(
        &self,
        resolver: Arc<dyn TenantResolver>,
        config: &TenancyConfig,
    ) -> bool {
        self.mode
            .fill(Mode::Ready {
                resolver,
                policy: config.policy(),
                statuses: config.statuses(),
            })
            .is_ok()
    }

    /// The configured statuses (defaults on an unwired router).
    #[must_use]
    pub fn statuses(&self) -> TenantStatuses {
        match self.mode.get() {
            Some(Mode::Ready { statuses, .. } | Mode::Disabled { statuses }) => *statuses,
            None => TenantStatuses::default(),
        }
    }

    /// The missing-tenant policy. A disabled or unwired router reports
    /// [`MissingTenantPolicy::Allow`]: `Option` extractors yield `None` rather
    /// than failing a whole app that deliberately turned tenancy off.
    #[must_use]
    pub fn policy(&self) -> MissingTenantPolicy {
        match self.mode.get() {
            Some(Mode::Ready { policy, .. }) => *policy,
            Some(Mode::Disabled { .. }) | None => MissingTenantPolicy::Allow,
        }
    }

    /// Whether a resolver is wired (`tenancy.enabled != false`).
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        matches!(self.mode.get(), Some(Mode::Ready { .. }))
    }

    /// The tenant already resolved for this request, if any.
    ///
    /// The memo written by [`resolve_parts`](Self::resolve_parts) — how a
    /// `#[managed]` resource acquired later in the same request avoids resolving
    /// again.
    #[must_use]
    pub fn memoized<'a>(head: &RequestHead<'a>) -> Option<&'a TenantId> {
        head.extension::<TenantId>()
    }

    /// Resolve the tenant, or `None` when the request carries none and the
    /// policy allows it.
    ///
    /// Honours the per-request memo; does not write it (the head is borrowed
    /// read-only) — see [`resolve_parts`](Self::resolve_parts).
    pub async fn try_resolve(
        &self,
        head: &RequestHead<'_>,
    ) -> Result<Option<TenantId>, HttpError> {
        if let Some(memo) = Self::memoized(head) {
            return Ok(Some(memo.clone()));
        }
        match self.mode.get() {
            None => Err(TenantError::NoResolver.into_http_error(TenantStatuses::default())),
            Some(Mode::Disabled { .. }) => Ok(None),
            Some(Mode::Ready {
                resolver,
                policy,
                statuses,
            }) => match resolver.resolve(head).await? {
                Some(tenant) => Ok(Some(tenant)),
                None => match policy {
                    MissingTenantPolicy::Allow => Ok(None),
                    MissingTenantPolicy::Reject => {
                        Err(TenantError::Unresolved.into_http_error(*statuses))
                    }
                },
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

    /// Resolve from request parts, **memoizing** the answer in
    /// `parts.extensions` so the rest of the request reuses it.
    ///
    /// This is the entry point extractors use. `#[managed]` resources, which
    /// only see a borrowed [`RequestHead`], read the memo back through
    /// [`memoized`](Self::memoized) / [`try_resolve`](Self::try_resolve).
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

    /// [`try_resolve`](Self::try_resolve) from request parts, memoizing a
    /// resolved tenant.
    pub async fn try_resolve_parts<S: Send + Sync>(
        &self,
        parts: &mut Parts,
        state: &S,
    ) -> Result<Option<TenantId>, HttpError> {
        if let Some(memo) = parts.extensions.get::<TenantId>() {
            return Ok(Some(memo.clone()));
        }
        // `RawPathParams` is cloned out of the extensions axum's router filled
        // in, so a `PathTenantResolver` works from an extractor exactly as it
        // does from a guard.
        let raw = r2e_core::http::extract::RawPathParams::from_request_parts(parts, state)
            .await
            .ok();
        let resolved = {
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
            self.try_resolve(&head).await?
        };
        if let Some(tenant) = &resolved {
            parts.extensions.insert(tenant.clone());
        }
        Ok(resolved)
    }
}

impl std::fmt::Debug for TenantRouter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = match self.mode.get() {
            None => "unwired",
            Some(Mode::Disabled { .. }) => "disabled",
            Some(Mode::Ready { .. }) => "ready",
        };
        f.debug_struct("TenantRouter").field("state", &state).finish()
    }
}
