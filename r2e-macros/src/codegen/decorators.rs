//! Decorator sets: guards/interceptors as graph-resolved values.
//!
//! Every `#[guard(...)]` / `#[pre_guard(...)]` / `#[intercept(...)]` site is
//! built **once**, inside `Controller::routes(state, core, ctx)`, through
//! `build_decorator::<_, Spec>(expr, ctx)` — never per request. The spec
//! type is the expression's **leading type path**:
//!
//! | attribute expression               | spec type    |
//! |------------------------------------|--------------|
//! | `MyGuard`                          | `MyGuard`    |
//! | `MyGuard("key")`                   | `MyGuard`    |
//! | `RolesGuard { .. }`                | `RolesGuard` |
//! | `RateLimit::per_user(5, 60)`       | `RateLimit`  |
//! | `Cache::ttl(30).group("x")`        | `Cache`      |
//! | `MyGuard = make_guard()` (escape)  | `MyGuard`    |
//!
//! The expression must evaluate either to the spec type itself (builder
//! chains return `Self`) or — for `#[derive(DecoratorBean)]` constructors
//! like `DbAuditLog::spec(..)` — to a companion spec with the same
//! `Product`/`Deps`; `build_decorator` enforces the equivalence. For each
//! method, a hidden struct holds the built products; one `Arc` of it is
//! captured by the handler closure. The specs' `Deps` are folded into
//! `Controller::Deps`, so a missing bean is a compile error at
//! `register_controller()`.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::crate_path::r2e_core_path;
use crate::routes_parsing::RoutesImplDef;
use crate::types::MethodDecorators;

/// Resolve a decorator expression to `(spec type path, value expression)`.
///
/// See the module table for the accepted shapes. Anything else (free
/// function calls, lowercase paths, literals…) needs the explicit
/// `SpecType = expr` form.
pub(crate) fn spec_type_of(expr: &syn::Expr) -> syn::Result<(syn::Path, syn::Expr)> {
    // Escape hatch: `SpecType = expr`.
    if let syn::Expr::Assign(assign) = expr {
        if let syn::Expr::Path(p) = assign.left.as_ref() {
            return Ok((p.path.clone(), (*assign.right).clone()));
        }
        return Err(syn::Error::new_spanned(
            &assign.left,
            "expected a type path left of `=` (e.g. `#[guard(MyGuard = make_guard())]`)",
        ));
    }

    // Walk builder-style method chains down to their base expression.
    let mut base = expr;
    while let syn::Expr::MethodCall(mc) = base {
        base = &mc.receiver;
    }

    let path = match base {
        // `MyGuard` — unit struct value.
        syn::Expr::Path(p) => Some(p.path.clone()),
        // `RolesGuard { .. }` — struct literal.
        syn::Expr::Struct(s) => Some(s.path.clone()),
        // `RateLimit::per_user(5, 60)` — associated constructor: drop the
        // final (function) segment. `MyGuard("key")` — a single-segment
        // uppercase call is treated as a tuple-struct constructor: the path
        // IS the spec type. The uppercase filter below rejects lowercase
        // free functions; an uppercase-named non-type (free fn, glob-
        // imported enum-variant ctor) slips through and errors downstream
        // at the `DecoratorSpec` bound instead of the "name it explicitly"
        // message.
        syn::Expr::Call(call) => match call.func.as_ref() {
            syn::Expr::Path(p) if p.path.segments.len() >= 2 => {
                let segments: Vec<syn::PathSegment> =
                    p.path.segments.iter().cloned().collect();
                Some(syn::Path {
                    leading_colon: p.path.leading_colon,
                    segments: segments[..segments.len() - 1].iter().cloned().collect(),
                })
            }
            syn::Expr::Path(p) => Some(p.path.clone()),
            _ => None,
        },
        _ => None,
    };

    let starts_uppercase = |path: &syn::Path| {
        path.segments
            .last()
            .map(|seg| {
                seg.ident
                    .to_string()
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_uppercase())
            })
            .unwrap_or(false)
    };

    match path {
        Some(path) if starts_uppercase(&path) => Ok((path, expr.clone())),
        _ => Err(syn::Error::new_spanned(
            expr,
            "cannot infer the decorator spec type from this expression; \
             name it explicitly: `#[guard(MyGuard = <expr>)]` / \
             `#[intercept(MyInterceptor = <expr>)]`",
        )),
    }
}

/// Whether every decorator expression's spec type is inferable. Closure
/// generation uses this to degrade to the same no-decorator shape as the
/// invocation function when extraction fails, so the only error the user
/// sees is the spec-type one (no arity-mismatch cascade).
pub(crate) fn all_specs_inferable<'a>(
    exprs: impl IntoIterator<Item = &'a syn::Expr>,
) -> bool {
    exprs.into_iter().all(|e| spec_type_of(e).is_ok())
}

/// A generated per-method decorator set: hidden struct + build function.
pub(crate) struct DecoSet {
    pub struct_ident: syn::Ident,
    pub ctor_ident: syn::Ident,
    /// Field idents for guard sites, in `guard_fns` order.
    pub guard_fields: Vec<syn::Ident>,
    /// Field idents for interceptor sites, controller-level first then
    /// method-level (execution order).
    pub intercept_fields: Vec<syn::Ident>,
}

impl DecoSet {
    pub fn ty(&self) -> &syn::Ident {
        &self.struct_ident
    }
}

/// Generate the decorator struct + constructor for one route/SSE/WS method.
///
/// `path_param_module` is the method's `mod path { const … }` block so spec
/// expressions can reference typed path-parameter descriptors
/// (`ProjectGuard::viewer(path::id)`); it is scoped to the constructor.
///
/// Returns `None` when the method has no guard/interceptor sites. On a spec
/// extraction failure the items contain a `compile_error!` and the set is
/// `None` (downstream codegen falls back to the no-decorator shape; the
/// error already fails the build).
pub(super) fn generate_deco_items(
    def: &RoutesImplDef,
    fn_ident: &syn::Ident,
    guard_exprs: &[syn::Expr],
    intercept_exprs: &[&syn::Expr],
    path_param_module: TokenStream,
) -> (TokenStream, Option<DecoSet>) {
    generate_named_deco_items(
        &def.controller_name,
        "Deco",
        fn_ident,
        guard_exprs,
        intercept_exprs,
        path_param_module,
    )
}

/// [`generate_deco_items`] with an explicit controller name and set-name
/// discriminant (`__R2e<kind>_<Controller>_<fn>`), for callers outside the
/// `#[routes]` HTTP path: scheduled tasks (`kind = "Sched"`) and gRPC
/// methods (`kind = "GrpcDeco"`). Distinct kinds keep the hidden items
/// collision free when one method name appears in several execution scopes.
pub(crate) fn generate_named_deco_items(
    controller_name: &syn::Ident,
    kind: &str,
    fn_ident: &syn::Ident,
    guard_exprs: &[syn::Expr],
    intercept_exprs: &[&syn::Expr],
    path_param_module: TokenStream,
) -> (TokenStream, Option<DecoSet>) {
    if guard_exprs.is_empty() && intercept_exprs.is_empty() {
        return (quote! {}, None);
    }

    let set = DecoSet {
        struct_ident: format_ident!("__R2e{}_{}_{}", kind, controller_name, fn_ident),
        ctor_ident: format_ident!(
            "__r2e_{}_{}_{}",
            kind.to_lowercase(),
            controller_name,
            fn_ident
        ),
        guard_fields: (0..guard_exprs.len())
            .map(|i| format_ident!("__g{}", i))
            .collect(),
        intercept_fields: (0..intercept_exprs.len())
            .map(|i| format_ident!("__i{}", i))
            .collect(),
    };

    let sites = set
        .guard_fields
        .iter()
        .zip(guard_exprs.iter())
        .chain(set.intercept_fields.iter().zip(intercept_exprs.iter().copied()));

    let mut field_decls: Vec<TokenStream> = Vec::new();
    let mut field_inits: Vec<TokenStream> = Vec::new();
    let krate = r2e_core_path();
    for (field, expr) in sites {
        let (spec_ty, value_expr) = match spec_type_of(expr) {
            Ok(split) => split,
            Err(err) => return (err.to_compile_error(), None),
        };
        field_decls.push(quote! {
            #field: <#spec_ty as #krate::DecoratorSpec>::Product
        });
        field_inits.push(quote! {
            #field: #krate::decorator::build_decorator::<_, #spec_ty>(#value_expr, __ctx)
        });
    }

    let struct_ident = &set.struct_ident;
    let ctor_ident = &set.ctor_ident;
    let items = quote! {
        #[allow(non_camel_case_types)]
        #[doc(hidden)]
        struct #struct_ident {
            #(#field_decls,)*
        }

        #[allow(non_snake_case)]
        #[doc(hidden)]
        fn #ctor_ident(__ctx: &#krate::beans::BeanContext) -> #struct_ident {
            #path_param_module
            #struct_ident {
                #(#field_inits,)*
            }
        }
    };
    (items, Some(set))
}

/// Generate the pre-auth decorator struct + constructor for one method.
/// Separate from [`generate_deco_items`] because pre-auth guards live in the
/// middleware closure, not the handler closure.
pub(super) fn generate_predeco_items(
    def: &RoutesImplDef,
    fn_ident: &syn::Ident,
    decorators: &MethodDecorators,
) -> (TokenStream, Option<DecoSet>) {
    if decorators.pre_auth_guard_fns.is_empty() {
        return (quote! {}, None);
    }

    let controller_name = &def.controller_name;
    let set = DecoSet {
        struct_ident: format_ident!("__R2ePreDeco_{}_{}", controller_name, fn_ident),
        ctor_ident: format_ident!("__r2e_predeco_{}_{}", controller_name, fn_ident),
        guard_fields: (0..decorators.pre_auth_guard_fns.len())
            .map(|i| format_ident!("__p{}", i))
            .collect(),
        intercept_fields: Vec::new(),
    };

    let mut field_decls: Vec<TokenStream> = Vec::new();
    let mut field_inits: Vec<TokenStream> = Vec::new();
    let krate = r2e_core_path();
    for (field, expr) in set
        .guard_fields
        .iter()
        .zip(decorators.pre_auth_guard_fns.iter())
    {
        let (spec_ty, value_expr) = match spec_type_of(expr) {
            Ok(split) => split,
            Err(err) => return (err.to_compile_error(), None),
        };
        field_decls.push(quote! {
            #field: <#spec_ty as #krate::DecoratorSpec>::Product
        });
        field_inits.push(quote! {
            #field: #krate::decorator::build_decorator::<_, #spec_ty>(#value_expr, __ctx)
        });
    }

    let struct_ident = &set.struct_ident;
    let ctor_ident = &set.ctor_ident;
    let items = quote! {
        #[allow(non_camel_case_types)]
        #[doc(hidden)]
        struct #struct_ident {
            #(#field_decls,)*
        }

        #[allow(non_snake_case)]
        #[doc(hidden)]
        fn #ctor_ident(__ctx: &#krate::beans::BeanContext) -> #struct_ident {
            #struct_ident {
                #(#field_inits,)*
            }
        }
    };
    (items, Some(set))
}

/// The hidden container holding every scheduled-method decorator set of one
/// controller. Stored in the core's `DecoSlot` at registration
/// (`scheduled_tasks_boxed`), read back (downcast by this type) both by the
/// scheduled method bodies (direct-call interception) and by the generated
/// task closures.
pub(super) fn sched_container_ident(controller_name: &syn::Ident) -> syn::Ident {
    format_ident!("__R2eSchedDecos_{}", controller_name)
}

/// The container field holding one scheduled method's prebuilt set.
pub(super) fn sched_field_ident(fn_name: &syn::Ident) -> syn::Ident {
    format_ident!("__deco_{}", fn_name)
}

/// The interceptor-site field idents of a scheduled method's decorator set,
/// recomputed from the site count. The method-emission pass (`wrapping.rs`)
/// and the registration pass (`controller_impl.rs`) both need them; the
/// idents are positional (`__i0..`), matching [`generate_named_deco_items`]'s
/// `DecoSet` layout.
pub(super) fn intercept_field_idents(count: usize) -> Vec<syn::Ident> {
    (0..count).map(|i| format_ident!("__i{}", i)).collect()
}

/// Wrap a body expression with the interceptor chain of a prebuilt decorator
/// set.
///
/// Interceptors are prebuilt fields of the method's decorator set; the caller
/// binds `__deco` to a `&` reference to the set (`Copy`), so the
/// `move || async move { ... }` closures capture it by copy and other
/// variables by move.
pub(crate) fn wrap_with_deco_interceptors(
    body: TokenStream,
    fn_name_str: &str,
    controller_name_str: &str,
    intercept_fields: &[syn::Ident],
    krate: &TokenStream,
) -> TokenStream {
    if intercept_fields.is_empty() {
        return body;
    }

    let intercept_ctx = quote! {
        #krate::InterceptorContext {
            method_name: #fn_name_str,
            controller_name: #controller_name_str,
        }
    };

    // Start with the innermost: the body wrapped in a move closure
    let mut wrapped = quote! {
        move || async move { #body }
    };

    // Wrap from innermost interceptor to second interceptor (skip outermost)
    for field in intercept_fields[1..].iter().rev() {
        wrapped = quote! {
            move || async move {
                #krate::Interceptor::around(
                    &__deco.#field,
                    #intercept_ctx,
                    #wrapped
                ).await
            }
        };
    }

    // Apply the outermost interceptor directly (not wrapped in a closure)
    let outermost = &intercept_fields[0];
    quote! {
        {
            #krate::Interceptor::around(
                &__deco.#outermost,
                #intercept_ctx,
                #wrapped
            ).await
        }
    }
}

/// The `Controller::Deps` fold: the core's `ContextConstruct::Deps` extended
/// with every decorator site's `<Spec as DecoratorSpec>::Deps`, deduplicated
/// by spec type. All lists are concrete, so the `TAppend` projections
/// normalize without extra bounds on the impl.
pub(super) fn controller_deps_fold(def: &RoutesImplDef) -> TokenStream {
    let krate = r2e_core_path();
    let name = &def.controller_name;

    let mut seen = std::collections::HashSet::new();
    let mut spec_paths: Vec<syn::Path> = Vec::new();
    let mut collect = |exprs: &[syn::Expr]| {
        for expr in exprs {
            if let Ok((path, _)) = spec_type_of(expr) {
                if seen.insert(quote!(#path).to_string()) {
                    spec_paths.push(path);
                }
            }
        }
    };

    // Controller-level interceptors are wired into HTTP route handlers and
    // scheduled tasks (SSE/WS do not run the interceptor chain), so their
    // deps only matter when at least one such method exists.
    if !def.route_methods.is_empty() || !def.scheduled_methods.is_empty() {
        collect(&def.controller_intercepts);
    }
    for rm in &def.route_methods {
        collect(&rm.decorators.guard_fns);
        collect(&rm.decorators.pre_auth_guard_fns);
        collect(&rm.decorators.intercept_fns);
    }
    // Scheduled methods run interceptors (built once at registration, from
    // the retained context, inside `scheduled_tasks_boxed`).
    for sm in &def.scheduled_methods {
        collect(&sm.intercept_fns);
    }
    // SSE/WS methods run guards (and pre-auth guards) but not interceptors.
    for sm in &def.sse_methods {
        collect(&sm.decorators.guard_fns);
        collect(&sm.decorators.pre_auth_guard_fns);
    }
    for wm in &def.ws_methods {
        collect(&wm.decorators.guard_fns);
        collect(&wm.decorators.pre_auth_guard_fns);
    }

    let mut deps = quote! { <#name as #krate::ContextConstruct>::Deps };
    for spec in spec_paths {
        deps = quote! {
            <#deps as #krate::type_list::TAppend<
                <#spec as #krate::DecoratorSpec>::Deps,
            >>::Output
        };
    }
    deps
}
