//! Parsing for the `#[grpc_routes(TraitPath)]` attribute macro.

use crate::parsing::member::{
    parse_decorators, parse_member, validate_impl_attrs, validate_member_attrs, MemberDef,
    MemberRules,
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
    /// gRPC methods with their attributes.
    pub methods: Vec<GrpcMethod>,
    /// Non-gRPC methods (helper functions, etc.) passed through unchanged.
    pub other_methods: Vec<syn::ImplItemFn>,
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

    let mut methods = Vec::new();
    let mut other_methods = Vec::new();

    for impl_item in item.items {
        let syn::ImplItem::Fn(method) = impl_item else {
            continue;
        };

        // Every async method in the impl block is considered a gRPC method
        // (matching the tonic trait). Non-async or methods without &self are helpers.
        let is_receiver = method
            .sig
            .inputs
            .first()
            .is_some_and(|arg| matches!(arg, syn::FnArg::Receiver(_)));

        if method.sig.asyncness.is_some() && is_receiver {
            methods.push(GrpcMethod {
                member: parse_member(method, &rules)?,
            });
        } else {
            // Pass-through helpers still can't carry gRPC-disallowed
            // markers: a sync `#[pre_destroy] fn close(&self)` would
            // otherwise be re-emitted verbatim and silently never run.
            validate_member_attrs(&method.attrs, &rules)?;
            other_methods.push(method);
        }
    }

    Ok(GrpcRoutesImplDef {
        controller_name,
        service_trait,
        descriptor,
        controller_guards,
        controller_intercepts,
        methods,
        other_methods,
    })
}
