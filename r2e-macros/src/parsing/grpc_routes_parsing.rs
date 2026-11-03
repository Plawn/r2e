//! Parsing for the `#[grpc_routes(TraitPath)]` attribute macro.

use proc_macro2::{Group, TokenStream, TokenTree};
use quote::ToTokens;

use crate::parsing::member::{
    parse_decorators, parse_member, reject_self_in_route_signature, validate_impl_attrs,
    validate_member_attrs, MemberDef, MemberRules,
};

/// Parsed arguments of the `#[grpc_routes(...)]` attribute:
/// `TraitPath` optionally followed by `, descriptor = <expr>`.
pub struct GrpcRoutesArgs {
    /// The tonic-generated service trait path.
    pub service_trait: syn::Path,
    /// Expression evaluating to the service's encoded `FileDescriptorSet`
    /// (`&'static [u8]`), used by gRPC server reflection.
    pub descriptor: Option<syn::Expr>,
}

impl syn::parse::Parse for GrpcRoutesArgs {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let service_trait: syn::Path = input.parse()?;
        let mut descriptor = None;
        while input.peek(syn::Token![,]) {
            input.parse::<syn::Token![,]>()?;
            if input.is_empty() {
                break;
            }
            let key: syn::Ident = input.parse()?;
            if key == "descriptor" {
                if descriptor.is_some() {
                    return Err(syn::Error::new_spanned(
                        &key,
                        "duplicate `descriptor` argument on #[grpc_routes]",
                    ));
                }
                input.parse::<syn::Token![=]>()?;
                descriptor = Some(input.parse()?);
            } else {
                return Err(syn::Error::new_spanned(
                    &key,
                    format!(
                        "unknown #[grpc_routes] argument `{key}`; expected `descriptor = <expr>`"
                    ),
                ));
            }
        }
        Ok(Self {
            service_trait,
            descriptor,
        })
    }
}

/// Parsed representation of a `#[grpc_routes(TraitPath)] impl Name { ... }` block.
pub struct GrpcRoutesImplDef {
    /// The controller struct name (e.g., `UserGrpcService`).
    pub controller_name: syn::Ident,
    /// The tonic-generated service trait path (e.g., `proto::user_service_server::UserService`).
    pub service_trait: syn::Path,
    /// Expression for the service's encoded `FileDescriptorSet`, if declared.
    pub descriptor: Option<syn::Expr>,
    /// Controller-level guards (`#[guard]`/`#[roles]`/`#[all_roles]` on the
    /// impl block) checked before every method's own guards.
    pub controller_guards: Vec<syn::Expr>,
    /// Controller-level interceptors applied to all methods.
    pub controller_intercepts: Vec<syn::Expr>,
    /// gRPC methods with their attributes. They run on the per-call request
    /// façade (`__R2eRequest_<Name>`), like HTTP handlers and MCP members.
    pub methods: Vec<GrpcMethod>,
    /// `#[request_helper]` helpers (marker stripped): emitted on the façade
    /// next to the methods, so they can read the request-scoped fields.
    pub facade_helpers: Vec<syn::ImplItemFn>,
    /// The tonic trait's associated types (`type FooStream = …;` for a
    /// server-streaming rpc), forwarded into the generated trait impl.
    pub assoc_types: Vec<syn::ImplItemType>,
    /// Every other item (sync / receiver-less helpers, consts), passed
    /// through unchanged onto the controller core's impl.
    pub other_items: Vec<syn::ImplItem>,
}

/// A single gRPC method: the shared member model (its name must match the
/// tonic trait method name; decorators are guards, roles and interceptors).
pub struct GrpcMethod {
    pub member: MemberDef,
}

/// Parse a `#[grpc_routes(TraitPath)] impl Name { ... }` block.
pub fn parse(args: GrpcRoutesArgs, item: syn::ItemImpl) -> syn::Result<GrpcRoutesImplDef> {
    let GrpcRoutesArgs {
        service_trait,
        descriptor,
    } = args;
    // Extract controller name from self type
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

    // Impl-level decorators (guards, roles, intercepts) are consumed by the
    // gRPC codegen. Everything else in the HTTP decorator family is rejected
    // up front — impl attrs are never re-emitted, so an unrejected
    // `#[pre_guard]` would silently no-op.
    let rules = MemberRules::grpc();
    validate_impl_attrs(&item.attrs, &rules)?;
    let controller_decorators = parse_decorators(&item.attrs, &rules)?;
    let controller_guards = controller_decorators.guard_fns;
    let controller_intercepts = controller_decorators.intercept_fns;

    // `Self::<Assoc>` in a method resolves against the tonic trait impl; the
    // method itself is emitted on the request façade, where `Self` is the
    // façade. Rewrite those paths to the declared type first.
    let assoc_types: Vec<syn::ImplItemType> = item
        .items
        .iter()
        .filter_map(|i| match i {
            syn::ImplItem::Type(t) => Some(t.clone()),
            _ => None,
        })
        .collect();

    let mut methods = Vec::new();
    let mut facade_helpers = Vec::new();
    let mut other_items = Vec::new();

    for impl_item in item.items {
        let method = match impl_item {
            syn::ImplItem::Fn(method) => method,
            syn::ImplItem::Type(_) => continue,
            other => {
                other_items.push(other);
                continue;
            }
        };

        // Every async method in the impl block is considered a gRPC method
        // (matching the tonic trait). Non-async or methods without &self are helpers.
        let is_receiver = method
            .sig
            .inputs
            .first()
            .is_some_and(|arg| matches!(arg, syn::FnArg::Receiver(_)));

        if method.sig.asyncness.is_some() && is_receiver {
            let method = substitute_self_assoc(method, &assoc_types)?;
            reject_self_in_route_signature(&method.sig, "#[grpc_routes]")?;
            methods.push(GrpcMethod {
                member: parse_member(method, &rules)?,
            });
        } else {
            // Pass-through helpers still can't carry gRPC-disallowed
            // markers: a sync `#[pre_destroy] fn close(&self)` would
            // otherwise be re-emitted verbatim and silently never run.
            validate_member_attrs(&method.attrs, &rules)?;
            if method
                .attrs
                .iter()
                .any(|a| a.path().is_ident("request_helper"))
            {
                let mut method = substitute_self_assoc(method, &assoc_types)?;
                reject_self_in_route_signature(&method.sig, "#[request_helper]")?;
                method.attrs.retain(|a| !a.path().is_ident("request_helper"));
                facade_helpers.push(method);
            } else {
                other_items.push(syn::ImplItem::Fn(method));
            }
        }
    }

    Ok(GrpcRoutesImplDef {
        controller_name,
        service_trait,
        descriptor,
        controller_guards,
        controller_intercepts,
        methods,
        facade_helpers,
        assoc_types,
        other_items,
    })
}

/// Rewrite every `Self::<Assoc>` path (one of the impl's associated types)
/// in `method` to the type the impl declares for it.
fn substitute_self_assoc(
    method: syn::ImplItemFn,
    assoc_types: &[syn::ImplItemType],
) -> syn::Result<syn::ImplItemFn> {
    if assoc_types.is_empty() {
        return Ok(method);
    }
    fn rewrite(tokens: TokenStream, assoc: &[syn::ImplItemType]) -> TokenStream {
        let tts: Vec<TokenTree> = tokens.into_iter().collect();
        let mut out: Vec<TokenTree> = Vec::with_capacity(tts.len());
        let mut i = 0;
        while i < tts.len() {
            if let TokenTree::Ident(id) = &tts[i]
                && id == "Self"
                && let (Some(TokenTree::Punct(c1)), Some(TokenTree::Punct(c2)), Some(TokenTree::Ident(name))) =
                    (tts.get(i + 1), tts.get(i + 2), tts.get(i + 3))
                && c1.as_char() == ':'
                && c2.as_char() == ':'
                && let Some(t) = assoc.iter().find(|t| t.ident == *name)
            {
                let ty = &t.ty;
                let group = Group::new(proc_macro2::Delimiter::None, ty.to_token_stream());
                out.push(TokenTree::Group(group));
                i += 4;
                continue;
            }
            match &tts[i] {
                TokenTree::Group(g) => {
                    let mut ng = Group::new(g.delimiter(), rewrite(g.stream(), assoc));
                    ng.set_span(g.span());
                    out.push(TokenTree::Group(ng));
                }
                other => out.push(other.clone()),
            }
            i += 1;
        }
        out.into_iter().collect()
    }
    syn::parse2(rewrite(method.to_token_stream(), assoc_types))
}
