//! Route plugin registry for centralised decorator parsing and attribute stripping.
//!
//! Each `RoutePlugin` knows how to parse one family of attributes (e.g. `#[roles]`,
//! `#[intercept]`) and populate the shared `MethodDecorators` struct.  The static
//! `PLUGINS` array is the single source of truth: adding a new decorator only
//! requires implementing `RoutePlugin` and appending it here.

use crate::extract::route::{
    extract_guard_fns, extract_intercept_fns, extract_layer_exprs, extract_middleware_fns,
    extract_pre_guard_fns, extract_returns, extract_roles, extract_status, extract_transactional,
    is_route_attr, is_sse_attr, is_ws_attr, roles_guard_expr,
};
use crate::types::MethodDecorators;

/// A decorator plugin that can parse one or more attributes into `MethodDecorators`.
pub trait RoutePlugin: Sync {
    /// Attribute identifiers consumed by this plugin (e.g. `["roles"]`).
    fn attr_names(&self) -> &'static [&'static str];

    /// Parse matching attributes and populate `decorators`.
    fn parse(
        &self,
        attrs: &[syn::Attribute],
        decorators: &mut MethodDecorators,
    ) -> syn::Result<()>;
}

// ── Plugin implementations ───────────────────────────────────────────────

struct RolesPlugin;
impl RoutePlugin for RolesPlugin {
    fn attr_names(&self) -> &'static [&'static str] {
        &["roles"]
    }
    fn parse(
        &self,
        attrs: &[syn::Attribute],
        decorators: &mut MethodDecorators,
    ) -> syn::Result<()> {
        decorators.roles = extract_roles(attrs)?;
        // Inject the RolesGuard at the front of the guard list so it runs first.
        if let Some(roles_guard) = roles_guard_expr(&decorators.roles) {
            decorators.guard_fns.push(roles_guard);
        }
        Ok(())
    }
}

struct GuardPlugin;
impl RoutePlugin for GuardPlugin {
    fn attr_names(&self) -> &'static [&'static str] {
        &["guard"]
    }
    fn parse(
        &self,
        attrs: &[syn::Attribute],
        decorators: &mut MethodDecorators,
    ) -> syn::Result<()> {
        decorators.guard_fns.extend(extract_guard_fns(attrs)?);
        Ok(())
    }
}

struct PreGuardPlugin;
impl RoutePlugin for PreGuardPlugin {
    fn attr_names(&self) -> &'static [&'static str] {
        &["pre_guard"]
    }
    fn parse(
        &self,
        attrs: &[syn::Attribute],
        decorators: &mut MethodDecorators,
    ) -> syn::Result<()> {
        decorators
            .pre_auth_guard_fns
            .extend(extract_pre_guard_fns(attrs)?);
        Ok(())
    }
}

struct TransactionalPlugin;
impl RoutePlugin for TransactionalPlugin {
    fn attr_names(&self) -> &'static [&'static str] {
        &["transactional"]
    }
    fn parse(
        &self,
        attrs: &[syn::Attribute],
        decorators: &mut MethodDecorators,
    ) -> syn::Result<()> {
        decorators.transactional = extract_transactional(attrs)?;
        Ok(())
    }
}

struct InterceptPlugin;
impl RoutePlugin for InterceptPlugin {
    fn attr_names(&self) -> &'static [&'static str] {
        &["intercept"]
    }
    fn parse(
        &self,
        attrs: &[syn::Attribute],
        decorators: &mut MethodDecorators,
    ) -> syn::Result<()> {
        decorators.intercept_fns = extract_intercept_fns(attrs)?;
        Ok(())
    }
}

struct MiddlewarePlugin;
impl RoutePlugin for MiddlewarePlugin {
    fn attr_names(&self) -> &'static [&'static str] {
        &["middleware"]
    }
    fn parse(
        &self,
        attrs: &[syn::Attribute],
        decorators: &mut MethodDecorators,
    ) -> syn::Result<()> {
        decorators.middleware_fns = extract_middleware_fns(attrs)?;
        Ok(())
    }
}

struct LayerPlugin;
impl RoutePlugin for LayerPlugin {
    fn attr_names(&self) -> &'static [&'static str] {
        &["layer"]
    }
    fn parse(
        &self,
        attrs: &[syn::Attribute],
        decorators: &mut MethodDecorators,
    ) -> syn::Result<()> {
        decorators.layer_exprs = extract_layer_exprs(attrs)?;
        Ok(())
    }
}

struct StatusPlugin;
impl RoutePlugin for StatusPlugin {
    fn attr_names(&self) -> &'static [&'static str] {
        &["status"]
    }
    fn parse(
        &self,
        attrs: &[syn::Attribute],
        decorators: &mut MethodDecorators,
    ) -> syn::Result<()> {
        decorators.status_override = extract_status(attrs)?;
        Ok(())
    }
}

struct ReturnsPlugin;
impl RoutePlugin for ReturnsPlugin {
    fn attr_names(&self) -> &'static [&'static str] {
        &["returns"]
    }
    fn parse(
        &self,
        attrs: &[syn::Attribute],
        decorators: &mut MethodDecorators,
    ) -> syn::Result<()> {
        decorators.returns_type = extract_returns(attrs)?;
        Ok(())
    }
}

// ── Registry ─────────────────────────────────────────────────────────────

/// Ordered registry of all decorator plugins for HTTP/SSE/WS routes.
/// **Ordering matters**: `RolesPlugin` runs before `GuardPlugin` so the
/// generated `RolesGuard` is inserted at the front of `guard_fns`.
static HTTP_PLUGINS: &[&dyn RoutePlugin] = &[
    &RolesPlugin,
    &GuardPlugin,
    &PreGuardPlugin,
    &TransactionalPlugin,
    &InterceptPlugin,
    &MiddlewarePlugin,
    &LayerPlugin,
    &StatusPlugin,
    &ReturnsPlugin,
];

/// Decorator plugins allowed for gRPC routes.
static GRPC_PLUGINS: &[&dyn RoutePlugin] = &[&InterceptPlugin];

const GRPC_DISALLOWED_ATTRS: &[&str] = &[
    "roles",
    "guard",
    "pre_guard",
    "transactional",
    "middleware",
    "layer",
];

/// Parse all decorator attributes into a single `MethodDecorators`.
pub fn parse_decorators(attrs: &[syn::Attribute]) -> syn::Result<MethodDecorators> {
    let mut decorators = MethodDecorators::default();
    for plugin in HTTP_PLUGINS {
        plugin.parse(attrs, &mut decorators)?;
    }
    Ok(decorators)
}

/// Parse decorators for gRPC routes (only `#[intercept]` is supported).
pub fn parse_grpc_decorators(attrs: &[syn::Attribute]) -> syn::Result<MethodDecorators> {
    validate_grpc_attrs(attrs)?;
    let mut decorators = MethodDecorators::default();
    for plugin in GRPC_PLUGINS {
        plugin.parse(attrs, &mut decorators)?;
    }
    Ok(decorators)
}

fn validate_grpc_attrs(attrs: &[syn::Attribute]) -> syn::Result<()> {
    for attr in attrs {
        for name in GRPC_DISALLOWED_ATTRS {
            if attr.path().is_ident(name) {
                return Err(syn::Error::new_spanned(
                    attr,
                    format!("#[{}] is not supported on #[grpc_routes] methods", name),
                ));
            }
        }
    }
    Ok(())
}

/// Collect all attribute names consumed by plugins.
pub fn all_decorator_attr_names() -> Vec<&'static str> {
    HTTP_PLUGINS
        .iter()
        .flat_map(|p| p.attr_names().iter().copied())
        .collect()
}

/// Strip all known decorator + route-kind attributes from a method's attribute list.
/// This replaces the old `strip_route_attrs()` function.
pub fn strip_known_attrs(attrs: Vec<syn::Attribute>) -> Vec<syn::Attribute> {
    let decorator_names = all_decorator_attr_names();
    attrs
        .into_iter()
        .filter(|a| {
            !is_route_attr(a)
                && !is_sse_attr(a)
                && !is_ws_attr(a)
                && !decorator_names.iter().any(|name| a.path().is_ident(name))
        })
        .collect()
}
