//! The one member model shared by `#[routes]`, `#[mcp_routes]` and
//! `#[grpc_routes]`.
//!
//! A *member* is a method a transport dispatches to: an HTTP route / `#[sse]`
//! / `#[ws]` method, an MCP `#[tool]`/`#[resource]`/`#[prompt]`/`#[completion]`,
//! or a gRPC RPC. Every transport parses the same things off it — the
//! decorator attributes, the `#[inject(identity)]` parameter, the doc comment
//! and `#[deprecated]` — and differs only in *which* decorators it accepts and
//! how it words its errors. That difference is data: a [`MemberRules`] value
//! per transport, consumed by [`parse_member`], [`parse_decorators`],
//! [`validate_member_attrs`] and [`validate_impl_attrs`].

use crate::extract::plugins::{RoutePlugin, GRPC_PLUGINS, HTTP_PLUGINS, MCP_PLUGINS};
use crate::extract::strip_known_attrs;
use crate::model::types::{IdentityParam, MethodDecorators};
use crate::parsing::identity_param::{parse_identity_param, strip_identity_param_attrs};

/// One parsed transport member: what every transport reads off a method,
/// before its own shape (HTTP path, MCP arguments, gRPC trait method) is
/// layered on top.
pub struct MemberDef {
    /// The method name.
    pub name: syn::Ident,
    /// The method item, decorator/marker attributes and the identity
    /// parameter's marker stripped.
    pub fn_item: syn::ImplItemFn,
    /// Parsed decorator attributes, restricted to the transport's plugins.
    pub decorators: MethodDecorators,
    /// The `#[inject(identity)]` handler parameter, if any.
    pub identity_param: Option<IdentityParam>,
    /// The doc comment, verbatim (line structure preserved).
    pub doc: Option<String>,
    /// Whether the method carries `#[deprecated]` (a standard attribute,
    /// left on the method).
    pub deprecated: bool,
}

/// Per-transport parsing rules for [`parse_member`] and friends.
pub struct MemberRules {
    /// The transport's attribute, as it appears in error messages
    /// (`#[grpc_routes]`, `#[mcp_routes]`).
    pub transport: &'static str,
    /// The transport's short name in impl-level errors (`gRPC`, `MCP`).
    pub label: &'static str,
    /// Decorator plugins the transport understands, in parse order (roles
    /// before guard: roles-derived guards come first in `guard_fns`).
    pub plugins: &'static [&'static dyn RoutePlugin],
    /// Attributes rejected on every method of the impl (members and helpers)
    /// and on the impl block itself.
    pub disallowed: &'static [&'static str],
    /// Transport-specific member markers stripped off the emitted method on
    /// top of the shared decorator / route-kind attributes.
    pub markers: &'static [&'static str],
    /// Whether the legacy `#[identity]` parameter marker is still accepted.
    pub allow_legacy_identity: bool,
    /// Error for a second `#[inject(identity)]` parameter.
    pub duplicate_identity: String,
}

/// Disallowed on `#[mcp_routes]` methods and impl blocks.
const MCP_DISALLOWED_ATTRS: &[&str] = &[
    // Pre-auth/anonymous/middleware are HTTP-pipeline concepts; MCP tools sit
    // behind one shared transport endpoint and have no per-tool middleware
    // stack. A struct-level `#[inject(identity)]` on an MCP service is
    // extracted once per HTTP request, before any member is dispatched, so
    // no member can opt out of it: mixed access is `Option<I>`.
    "anonymous",
    "pre_guard",
    "middleware",
    "layer",
    // HTTP response shaping has no MCP equivalent.
    "status",
    "returns",
    // Lifecycle / transverse markers are not wired for MCP services (same
    // rationale as gRPC): left unrejected they would silently never run.
    "pre_destroy",
    "post_construct",
    "on_start",
    "scheduled",
    "consumer",
    "async_exec",
];

/// Disallowed on `#[grpc_routes]` methods and impl blocks.
const GRPC_DISALLOWED_ATTRS: &[&str] = &[
    // Pre-auth/middleware are HTTP-pipeline concepts: a gRPC method has no
    // per-method tower stack. `#[anonymous]` stays out too: a struct-level
    // identity authenticates every method of the service, and a service
    // mixing public and protected methods declares `Option<I>` (struct
    // field or method parameter) instead of opting methods out.
    "anonymous",
    "pre_guard",
    "middleware",
    "layer",
    // Lifecycle / transverse markers are not wired for gRPC services. Left
    // unrejected they either silently never run (sync shapes drop into
    // `other_methods`) or die with a confusing E0407 "not a member of trait"
    // (async shapes land in the tonic trait impl). Reject them up front.
    "pre_destroy",
    "post_construct",
    "on_start",
    "scheduled",
    "consumer",
    "async_exec",
];

/// The MCP member markers, stripped off the emitted method.
pub const MCP_MEMBER_MARKERS: &[&str] = &["tool", "resource", "prompt", "completion"];

impl MemberRules {
    /// `#[routes]`: every HTTP decorator; impl-level restrictions live in
    /// `routes_parsing::reject_unsupported_impl_attrs`.
    pub fn http() -> Self {
        Self {
            transport: "#[routes]",
            label: "HTTP",
            plugins: HTTP_PLUGINS,
            disallowed: &[],
            markers: &[],
            allow_legacy_identity: false,
            duplicate_identity: "only one #[inject(identity)] parameter is allowed per handler\n\n\
                 hint: extract additional fields from the single identity:\n\
                 \n  async fn handler(&self, #[inject(identity)] user: AuthenticatedUser) {\n\
                 \n      let email = user.email();\n      let roles = user.roles();\n  }"
                .to_string(),
        }
    }

    /// `#[grpc_routes]`: guards, roles and interceptors.
    pub fn grpc() -> Self {
        Self {
            transport: "#[grpc_routes]",
            label: "gRPC",
            plugins: GRPC_PLUGINS,
            disallowed: GRPC_DISALLOWED_ATTRS,
            markers: &[],
            allow_legacy_identity: true,
            duplicate_identity:
                "only one #[inject(identity)] parameter is allowed per gRPC handler".to_string(),
        }
    }

    /// `#[mcp_routes]`: guards, roles and interceptors. `marker` is the
    /// member family's attribute name (`tool`, `resource`, …), named in the
    /// duplicate-identity error.
    pub fn mcp(marker: &str) -> Self {
        Self {
            transport: "#[mcp_routes]",
            label: "MCP",
            plugins: MCP_PLUGINS,
            disallowed: MCP_DISALLOWED_ATTRS,
            markers: MCP_MEMBER_MARKERS,
            allow_legacy_identity: true,
            duplicate_identity: format!(
                "only one #[inject(identity)] parameter is allowed per #[{marker}] method"
            ),
        }
    }
}

/// Parse a member method: decorators (validated against `rules`), doc
/// comment, `#[deprecated]`, then strip the consumed attributes and extract
/// the `#[inject(identity)]` parameter.
pub fn parse_member(mut method: syn::ImplItemFn, rules: &MemberRules) -> syn::Result<MemberDef> {
    let all_attrs = std::mem::take(&mut method.attrs);
    let decorators = parse_decorators(&all_attrs, rules)?;
    let doc = full_doc_text(&all_attrs);
    let deprecated = crate::extract::is_deprecated(&all_attrs);
    method.attrs = strip_known_attrs(all_attrs)
        .into_iter()
        .filter(|a| !rules.markers.iter().any(|m| a.path().is_ident(m)))
        .collect();
    let identity_param = extract_identity_param(&mut method, rules)?;
    Ok(MemberDef {
        name: method.sig.ident.clone(),
        fn_item: method,
        decorators,
        identity_param,
        doc,
        deprecated,
    })
}

/// Parse the decorator attributes of a member (or of the impl block) through
/// the transport's plugins, after rejecting the transport's disallowed
/// attributes.
pub fn parse_decorators(
    attrs: &[syn::Attribute],
    rules: &MemberRules,
) -> syn::Result<MethodDecorators> {
    validate_member_attrs(attrs, rules)?;
    let mut decorators = MethodDecorators::default();
    for plugin in rules.plugins {
        plugin.parse(attrs, &mut decorators)?;
    }
    Ok(decorators)
}

/// Reject attributes the transport does not support on its methods.
///
/// Runs for **every** method of the impl — members (via [`parse_decorators`])
/// and pass-through helpers — so a disallowed marker on a helper is caught
/// instead of being silently dropped.
pub fn validate_member_attrs(attrs: &[syn::Attribute], rules: &MemberRules) -> syn::Result<()> {
    if let Some((attr, name)) = find_disallowed(attrs, rules) {
        return Err(syn::Error::new_spanned(
            attr,
            format!(
                "#[{}] is not supported on {} methods",
                name, rules.transport
            ),
        ));
    }
    Ok(())
}

/// Reject disallowed attributes on the transport's **impl block** itself.
/// The impl level supports `#[intercept]`, `#[guard]`, `#[roles]` and
/// `#[all_roles]` (applied to every member). Left unrejected, the rest would
/// be silently dropped.
pub fn validate_impl_attrs(attrs: &[syn::Attribute], rules: &MemberRules) -> syn::Result<()> {
    if let Some((attr, name)) = find_disallowed(attrs, rules) {
        return Err(syn::Error::new_spanned(
            attr,
            format!(
                "#[{}] is not supported on a {} impl block — \
                 {} controller-level decorators support #[intercept], \
                 #[guard], #[roles] and #[all_roles] only",
                name, rules.transport, rules.label
            ),
        ));
    }
    Ok(())
}

fn find_disallowed<'a>(
    attrs: &'a [syn::Attribute],
    rules: &MemberRules,
) -> Option<(&'a syn::Attribute, &'static str)> {
    attrs.iter().find_map(|attr| {
        rules
            .disallowed
            .iter()
            .find(|name| attr.path().is_ident(name))
            .map(|name| (attr, *name))
    })
}

/// Detect the `#[inject(identity)]` handler parameter (index among typed
/// params, `&self` excluded) and strip its marker.
fn extract_identity_param(
    method: &mut syn::ImplItemFn,
    rules: &MemberRules,
) -> syn::Result<Option<IdentityParam>> {
    let mut identity_param = None;
    let mut param_idx = 0usize;
    for arg in method.sig.inputs.iter_mut() {
        if let syn::FnArg::Typed(pat_type) = arg {
            if let Some(identity) =
                parse_identity_param(pat_type, param_idx, rules.allow_legacy_identity)?
            {
                if identity_param.is_some() {
                    return Err(syn::Error::new_spanned(pat_type, &rules.duplicate_identity));
                }
                strip_identity_param_attrs(pat_type, rules.allow_legacy_identity);
                identity_param = Some(identity);
            }
            param_idx += 1;
        }
    }
    Ok(identity_param)
}

/// The full doc comment of a method, verbatim: one string with the original
/// line structure (each line stripped of the single leading space rustdoc
/// inserts), leading/trailing blank lines trimmed.
fn full_doc_text(attrs: &[syn::Attribute]) -> Option<String> {
    let lines: Vec<String> = attrs
        .iter()
        .filter_map(|attr| {
            if !attr.path().is_ident("doc") {
                return None;
            }
            if let syn::Meta::NameValue(nv) = &attr.meta
                && let syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Str(lit_str),
                    ..
                }) = &nv.value
            {
                let raw = lit_str.value();
                return Some(raw.strip_prefix(' ').unwrap_or(&raw).to_string());
            }
            None
        })
        .collect();
    let text = lines.join("\n");
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// Reject member methods that mention `Self` in their public signature.
///
/// Members run on the generated request façade (`impl __R2eRequest_<Name>`),
/// so `Self` in a parameter or return type would silently change meaning from
/// the controller core to the hidden façade. Until façade semantics for such
/// signatures are deliberately designed, reject them with a single targeted
/// error. The `&self` receiver is fine — only type positions are checked.
pub(crate) fn reject_self_in_route_signature(sig: &syn::Signature, kind: &str) -> syn::Result<()> {
    fn ty_mentions_self(ty: &syn::Type) -> bool {
        use quote::ToTokens;
        ty.to_token_stream()
            .into_iter()
            .any(|tt| matches!(&tt, proc_macro2::TokenTree::Ident(i) if i == "Self"))
    }

    for arg in sig.inputs.iter() {
        if let syn::FnArg::Typed(pt) = arg
            && ty_mentions_self(&pt.ty)
        {
            return Err(syn::Error::new_spanned(
                &pt.ty,
                format!(
                    "{kind} methods cannot expose `Self` in their signature\n\n\
                         {kind} methods run on the generated request façade, where `Self` no \
                         longer refers to the controller. Name the controller type explicitly \
                         instead of using `Self`.",
                ),
            ));
        }
    }
    if let syn::ReturnType::Type(_, ty) = &sig.output
        && ty_mentions_self(ty)
    {
        return Err(syn::Error::new_spanned(
            ty,
            format!(
                "{kind} methods cannot expose `Self` in their return type\n\n\
                     {kind} methods run on the generated request façade, where `Self` no \
                     longer refers to the controller. Name the controller type explicitly \
                     instead of using `Self`.",
            ),
        ));
    }
    Ok(())
}
