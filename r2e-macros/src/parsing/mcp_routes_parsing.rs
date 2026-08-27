//! Parsing for the `#[mcp_routes]` attribute macro.
//!
//! `#[mcp_routes] impl Name { ... }` turns every `#[tool]`-marked async
//! `&self` method into an MCP tool. Other methods pass through unchanged
//! (after attribute validation). The impl block itself may carry
//! `#[intercept]`, `#[guard]`, `#[roles]` and `#[all_roles]`, applied to
//! every tool.

use crate::extract::*;
use crate::model::types::{IdentityParam, MethodDecorators};
use crate::parsing::controller_parsing::has_identity_qualifier;
use crate::util::type_utils::unwrap_option_type;

/// Metadata parsed from a `#[tool(...)]` attribute.
#[derive(Default)]
pub struct McpToolMeta {
    /// Wire name override (`name = "..."`); defaults to the method name.
    pub name: Option<String>,
    /// Human-readable title (`title = "..."`).
    pub title: Option<String>,
    /// Description override (`description = "..."`); defaults to the doc
    /// comment.
    pub description: Option<String>,
    /// `read_only` / `read_only = <bool>` annotation hint.
    pub read_only: Option<bool>,
    /// `destructive` / `destructive = <bool>` annotation hint.
    pub destructive: Option<bool>,
    /// `idempotent` / `idempotent = <bool>` annotation hint.
    pub idempotent: Option<bool>,
    /// `open_world` / `open_world = <bool>` annotation hint.
    pub open_world: Option<bool>,
    /// Scopes the caller must ALL hold (`scopes = "a,b"` or
    /// `scopes = ["a", "b"]`).
    pub scopes: Vec<String>,
    /// Scopes of which the caller must hold at least one (`any_scopes = ...`,
    /// same forms as `scopes`).
    pub any_scopes: Vec<String>,
}

/// One (typed, non-receiver) parameter of a tool method, in declaration
/// order — the codegen re-emits the call arguments in this order.
pub enum McpToolArg {
    /// `#[inject(identity)] user: I` or `Option<I>`.
    Identity(IdentityParam),
    /// `Params(p): Params<T>` — the inner `T` (deserialized tool arguments).
    Params(syn::Type),
    /// A `ToolCall` parameter (raw call metadata).
    Call,
    /// A `CancelToken` parameter.
    Cancel,
}

/// A single `#[tool]` method with parsed attributes.
pub struct McpTool {
    /// Method name.
    pub name: syn::Ident,
    /// `#[tool(...)]` metadata.
    pub meta: McpToolMeta,
    /// The method's doc comment, verbatim (line structure preserved) — MCP
    /// tool descriptions have no summary/body split.
    pub doc_text: Option<String>,
    /// Parsed decorator attributes (`#[roles]`/`#[all_roles]`/`#[guard]`/
    /// `#[intercept]`).
    pub decorators: MethodDecorators,
    /// Typed parameters in declaration order.
    pub args: Vec<McpToolArg>,
    /// The original method item (with known attrs stripped).
    pub fn_item: syn::ImplItemFn,
}

impl McpTool {
    /// The wire tool name: `#[tool(name = ...)]` override or the method name.
    pub fn tool_name(&self) -> String {
        self.meta
            .name
            .clone()
            .unwrap_or_else(|| self.name.to_string())
    }

    /// The identity parameter, if any.
    pub fn identity_param(&self) -> Option<&IdentityParam> {
        self.args.iter().find_map(|a| match a {
            McpToolArg::Identity(p) => Some(p),
            _ => None,
        })
    }

    /// The `Params<T>` inner type, if any.
    pub fn params_type(&self) -> Option<&syn::Type> {
        self.args.iter().find_map(|a| match a {
            McpToolArg::Params(t) => Some(t),
            _ => None,
        })
    }
}

/// Parsed representation of an `#[mcp_routes] impl Name { ... }` block.
pub struct McpRoutesImplDef {
    /// The service struct name (e.g., `MathTools`).
    pub controller_name: syn::Ident,
    /// Controller-level guards (impl-level `#[roles]`/`#[all_roles]`/
    /// `#[guard]`), prepended to every tool's guard list.
    pub controller_guards: Vec<syn::Expr>,
    /// Controller-level interceptors, applied outermost on every tool.
    pub controller_intercepts: Vec<syn::Expr>,
    /// Impl-level `#[roles(...)]` literals, recorded into every tool's
    /// `ToolRequirements` (enforcement stays with the generated guard).
    pub controller_roles: Vec<String>,
    /// Impl-level `#[all_roles(...)]` literals, recorded likewise.
    pub controller_all_roles: Vec<String>,
    /// `#[tool]` methods.
    pub tools: Vec<McpTool>,
    /// Non-tool methods (helpers), passed through unchanged.
    pub other_methods: Vec<syn::ImplItemFn>,
}

/// Parse the `#[tool(...)]` attribute arguments.
fn parse_tool_meta(attr: &syn::Attribute) -> syn::Result<McpToolMeta> {
    let mut meta = McpToolMeta::default();
    // Bare `#[tool]` has no argument list.
    if matches!(attr.meta, syn::Meta::Path(_)) {
        return Ok(meta);
    }
    attr.parse_nested_meta(|nested| {
        let ident = nested
            .path
            .get_ident()
            .ok_or_else(|| nested.error("expected an identifier"))?
            .to_string();
        let parse_str = |nested: &syn::meta::ParseNestedMeta| -> syn::Result<String> {
            let lit: syn::LitStr = nested.value()?.parse()?;
            Ok(lit.value())
        };
        // Flags: bare = true, `= <bool>` accepted.
        let parse_flag = |nested: &syn::meta::ParseNestedMeta| -> syn::Result<bool> {
            if nested.input.peek(syn::Token![=]) {
                let lit: syn::LitBool = nested.value()?.parse()?;
                Ok(lit.value())
            } else {
                Ok(true)
            }
        };
        // Scope lists: `= "a,b"` (comma/whitespace separated, the OAuth
        // `scope` parameter shape) or `= ["a", "b"]`.
        let parse_scope_list = |nested: &syn::meta::ParseNestedMeta| -> syn::Result<Vec<String>> {
            let value = nested.value()?;
            let scopes: Vec<String> = if value.peek(syn::token::Bracket) {
                let content;
                syn::bracketed!(content in value);
                content
                    .parse_terminated(<syn::LitStr as syn::parse::Parse>::parse, syn::Token![,])?
                    .iter()
                    .map(|lit| lit.value().trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            } else {
                let lit: syn::LitStr = value.parse()?;
                lit.value()
                    .split(|c: char| c == ',' || c.is_whitespace())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(String::from)
                    .collect()
            };
            if scopes.is_empty() {
                return Err(nested.error("expected at least one scope"));
            }
            Ok(scopes)
        };
        match ident.as_str() {
            "name" => meta.name = Some(parse_str(&nested)?),
            "title" => meta.title = Some(parse_str(&nested)?),
            "description" => meta.description = Some(parse_str(&nested)?),
            "read_only" => meta.read_only = Some(parse_flag(&nested)?),
            "destructive" => meta.destructive = Some(parse_flag(&nested)?),
            "idempotent" => meta.idempotent = Some(parse_flag(&nested)?),
            "open_world" => meta.open_world = Some(parse_flag(&nested)?),
            "scopes" => meta.scopes = parse_scope_list(&nested)?,
            "any_scopes" => meta.any_scopes = parse_scope_list(&nested)?,
            other => {
                return Err(nested.error(format!(
                    "unknown #[tool] argument `{other}`; expected `name`, `title`, \
                     `description`, `read_only`, `destructive`, `idempotent`, \
                     `open_world`, `scopes` or `any_scopes`"
                )))
            }
        }
        Ok(())
    })?;
    Ok(meta)
}

/// Return `true` when the type path's last segment matches `ident` (bare or
/// fully qualified spelling).
fn last_segment_is(ty: &syn::Type, ident: &str) -> bool {
    match ty {
        syn::Type::Path(tp) => tp
            .path
            .segments
            .last()
            .is_some_and(|s| s.ident == ident),
        _ => false,
    }
}

/// If `ty` is `Params<T>` (by last path segment), return `T`.
fn unwrap_params_type(ty: &syn::Type) -> Option<syn::Type> {
    let syn::Type::Path(tp) = ty else { return None };
    let seg = tp.path.segments.last()?;
    if seg.ident != "Params" {
        return None;
    }
    let syn::PathArguments::AngleBracketed(args) = &seg.arguments else {
        return None;
    };
    args.args.iter().find_map(|a| match a {
        syn::GenericArgument::Type(t) => Some(t.clone()),
        _ => None,
    })
}

/// The full doc comment of a method, verbatim: one string with the original
/// line structure (each line stripped of the single leading space rustdoc
/// inserts), leading/trailing blank lines trimmed. MCP tool descriptions are
/// a single field — no OpenAPI-style summary/body split.
fn full_doc_text(attrs: &[syn::Attribute]) -> Option<String> {
    let lines: Vec<String> = attrs
        .iter()
        .filter_map(|attr| {
            if !attr.path().is_ident("doc") {
                return None;
            }
            if let syn::Meta::NameValue(nv) = &attr.meta {
                if let syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Str(lit_str),
                    ..
                }) = &nv.value
                {
                    let raw = lit_str.value();
                    return Some(raw.strip_prefix(' ').unwrap_or(&raw).to_string());
                }
            }
            None
        })
        .collect();
    let text = lines.join("\n");
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// Classify the typed parameters of a tool method, in order.
fn classify_args(method: &mut syn::ImplItemFn) -> syn::Result<Vec<McpToolArg>> {
    let mut args = Vec::new();
    let mut has_identity = false;
    let mut has_params = false;
    let mut has_call = false;
    let mut has_cancel = false;

    for arg in method.sig.inputs.iter_mut() {
        let syn::FnArg::Typed(pat_type) = arg else {
            continue;
        };
        let is_identity = pat_type.attrs.iter().any(|a| {
            (a.path().is_ident("inject") && has_identity_qualifier(a))
                || a.path().is_ident("identity")
        });
        if is_identity {
            if has_identity {
                return Err(syn::Error::new_spanned(
                    pat_type,
                    "only one #[inject(identity)] parameter is allowed per #[tool] method",
                ));
            }
            has_identity = true;
            let declared_ty = (*pat_type.ty).clone();
            let (inner_ty, is_optional) = match unwrap_option_type(&declared_ty) {
                Some(inner) => (inner.clone(), true),
                None => (declared_ty, false),
            };
            pat_type.attrs.retain(|a| {
                !((a.path().is_ident("inject") && has_identity_qualifier(a))
                    || a.path().is_ident("identity"))
            });
            args.push(McpToolArg::Identity(IdentityParam {
                index: args.len(),
                ty: inner_ty,
                is_optional,
            }));
            continue;
        }
        if let Some(inner) = unwrap_params_type(&pat_type.ty) {
            if has_params {
                return Err(syn::Error::new_spanned(
                    pat_type,
                    "only one Params<T> parameter is allowed per #[tool] method",
                ));
            }
            has_params = true;
            args.push(McpToolArg::Params(inner));
            continue;
        }
        if last_segment_is(&pat_type.ty, "ToolCall") {
            if has_call {
                return Err(syn::Error::new_spanned(
                    pat_type,
                    "only one ToolCall parameter is allowed per #[tool] method",
                ));
            }
            has_call = true;
            args.push(McpToolArg::Call);
            continue;
        }
        if last_segment_is(&pat_type.ty, "CancelToken") {
            if has_cancel {
                return Err(syn::Error::new_spanned(
                    pat_type,
                    "only one CancelToken parameter is allowed per #[tool] method",
                ));
            }
            has_cancel = true;
            args.push(McpToolArg::Cancel);
            continue;
        }
        return Err(syn::Error::new_spanned(
            pat_type,
            "unsupported #[tool] parameter: expected `Params<T>` (tool arguments), \
             `#[inject(identity)] user: I` (or `Option<I>`), `ToolCall`, or `CancelToken`. \
             Beans and config go on the struct (`#[inject]`/`#[config]` fields)",
        ));
    }
    Ok(args)
}

/// Parse an `#[mcp_routes] impl Name { ... }` block.
pub fn parse(item: syn::ItemImpl) -> syn::Result<McpRoutesImplDef> {
    let controller_name = match *item.self_ty {
        syn::Type::Path(ref type_path) => type_path
            .path
            .segments
            .last()
            .ok_or_else(|| syn::Error::new_spanned(&item.self_ty, "expected type name"))?
            .ident
            .clone(),
        _ => {
            return Err(syn::Error::new_spanned(
                &item.self_ty,
                "expected a type path",
            ))
        }
    };

    // Impl-level decorators: guards/roles/intercepts allowed, everything else
    // rejected up front (impl attrs are never re-emitted, so an unrejected
    // marker would silently no-op).
    validate_mcp_impl_attrs(&item.attrs)?;
    let controller_decorators = parse_mcp_decorators(&item.attrs)?;
    let controller_guards = controller_decorators.guard_fns;
    let controller_intercepts = controller_decorators.intercept_fns;
    let controller_roles = controller_decorators.roles;
    let controller_all_roles = controller_decorators.all_roles;

    let mut tools = Vec::new();
    let mut other_methods = Vec::new();
    let mut seen_names: std::collections::HashMap<String, syn::Ident> =
        std::collections::HashMap::new();

    for impl_item in item.items {
        let syn::ImplItem::Fn(mut method) = impl_item else {
            continue; // skip non-method items (consts, types)
        };
        let all_attrs = std::mem::take(&mut method.attrs);
        let tool_attr = all_attrs.iter().find(|a| a.path().is_ident("tool"));

        let Some(tool_attr) = tool_attr else {
            // Pass-through helpers still can't carry disallowed markers, and a
            // decorator without #[tool] would silently never run.
            validate_mcp_attrs(&all_attrs)?;
            for attr in &all_attrs {
                for name in ["roles", "all_roles", "guard", "intercept"] {
                    if attr.path().is_ident(name) {
                        return Err(syn::Error::new_spanned(
                            attr,
                            format!(
                                "#[{name}] on a method without #[tool] does nothing in an \
                                 #[mcp_routes] impl — add #[tool] or remove the decorator"
                            ),
                        ));
                    }
                }
            }
            method.attrs = all_attrs;
            other_methods.push(method);
            continue;
        };

        // Tool methods must be async with a &self receiver.
        let is_ref_receiver = method
            .sig
            .inputs
            .first()
            .is_some_and(|arg| matches!(arg, syn::FnArg::Receiver(r) if r.reference.is_some() && r.mutability.is_none()));
        if method.sig.asyncness.is_none() || !is_ref_receiver {
            return Err(syn::Error::new_spanned(
                &method.sig,
                "#[tool] methods must be `async fn` taking `&self`",
            ));
        }

        let meta = parse_tool_meta(tool_attr)?;
        let decorators = parse_mcp_decorators(&all_attrs)?;
        let doc_text = full_doc_text(&all_attrs);

        // strip_known_attrs keeps doc comments and unknown attrs; #[tool] is
        // MCP-specific, strip it explicitly.
        method.attrs = strip_known_attrs(all_attrs)
            .into_iter()
            .filter(|a| !a.path().is_ident("tool"))
            .collect();

        let args = classify_args(&mut method)?;
        let name = method.sig.ident.clone();

        let tool = McpTool {
            name,
            meta,
            doc_text,
            decorators,
            args,
            fn_item: method,
        };
        if let Some(prev) = seen_names.insert(tool.tool_name(), tool.name.clone()) {
            return Err(syn::Error::new_spanned(
                &tool.name,
                format!(
                    "duplicate tool name `{}` (also produced by method `{prev}`); \
                     use #[tool(name = \"...\")] to disambiguate",
                    tool.tool_name()
                ),
            ));
        }
        tools.push(tool);
    }

    Ok(McpRoutesImplDef {
        controller_name,
        controller_guards,
        controller_intercepts,
        controller_roles,
        controller_all_roles,
        tools,
        other_methods,
    })
}
