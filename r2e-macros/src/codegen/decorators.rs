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

use crate::util::crate_path::r2e_core_path;
use crate::parsing::routes_parsing::RoutesImplDef;
use crate::model::types::MethodDecorators;

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
                let segments: Vec<syn::PathSegment> = p.path.segments.iter().cloned().collect();
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
pub(crate) fn all_specs_inferable<'a>(exprs: impl IntoIterator<Item = &'a syn::Expr>) -> bool {
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

    let sites = set.guard_fields.iter().zip(guard_exprs.iter()).chain(
        set.intercept_fields
            .iter()
            .zip(intercept_exprs.iter().copied()),
    );

    match emit_deco_struct(&set.struct_ident, &set.ctor_ident, sites, path_param_module) {
        Ok(items) => (items, Some(set)),
        Err(err) => (err, None),
    }
}

/// Shared emitter for a hidden decorator-product struct + its `BeanContext`
/// constructor: one field per (ident, spec expr) site, initialized via
/// `build_decorator`. `path_param_module` is spliced into the ctor body (pass
/// empty tokens when unused). `Err` carries the `spec_type_of` compile_error
/// for the first non-inferable spec.
fn emit_deco_struct<'a, 'b>(
    struct_ident: &syn::Ident,
    ctor_ident: &syn::Ident,
    sites: impl Iterator<Item = (&'a syn::Ident, &'b syn::Expr)>,
    path_param_module: TokenStream,
) -> Result<TokenStream, TokenStream> {
    let mut field_decls: Vec<TokenStream> = Vec::new();
    let mut field_inits: Vec<TokenStream> = Vec::new();
    let krate = r2e_core_path();
    for (field, expr) in sites {
        let (spec_ty, value_expr) = match spec_type_of(expr) {
            Ok(split) => split,
            Err(err) => return Err(err.to_compile_error()),
        };
        field_decls.push(quote! {
            #field: <#spec_ty as #krate::DecoratorSpec>::Product
        });
        field_inits.push(quote! {
            #field: #krate::decorator::build_decorator::<_, #spec_ty>(#value_expr, __ctx)
        });
    }

    Ok(quote! {
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
    })
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
pub(crate) fn intercept_field_idents(count: usize) -> Vec<syn::Ident> {
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
    let refs: Vec<TokenStream> = intercept_fields
        .iter()
        .map(|f| quote! { &__deco.#f })
        .collect();
    wrap_with_interceptor_refs(body, fn_name_str, controller_name_str, &refs, krate)
}

/// Like [`wrap_with_deco_interceptors`] but the interceptor references are
/// supplied explicitly (outermost first), each already a `&`-reference
/// expression yielding the built `Interceptor` product.
///
/// This is the split-source form used when controller-level (impl-level)
/// interceptors are built **once per controller** and shared across routes /
/// transverse methods: the controller-level refs point into a single shared
/// set (`&__ctrl_deco.__ci0`), while the method-level refs point into the
/// per-method set (`&__deco.__i0`). The full ordered list keeps impl-level
/// interceptors outermost, then method-level ones — unchanged execution order.
pub(crate) fn wrap_with_interceptor_refs(
    body: TokenStream,
    fn_name_str: &str,
    controller_name_str: &str,
    interceptor_refs: &[TokenStream],
    krate: &TokenStream,
) -> TokenStream {
    if interceptor_refs.is_empty() {
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
    for r in interceptor_refs[1..].iter().rev() {
        wrapped = quote! {
            move || async move {
                #krate::Interceptor::around(
                    #r,
                    #intercept_ctx,
                    #wrapped
                ).await
            }
        };
    }

    // Apply the outermost interceptor directly (not wrapped in a closure)
    let outermost = &interceptor_refs[0];
    quote! {
        {
            #krate::Interceptor::around(
                #outermost,
                #intercept_ctx,
                #wrapped
            ).await
        }
    }
}

/// A generated per-controller **shared** decorator set: hidden struct + build
/// function holding the controller-level (impl-level) `#[intercept]`,
/// `#[guard]`/`#[roles]`/`#[all_roles]`, and `#[pre_guard]` products, built
/// **once per controller** (not once per route/method) so a stateful
/// impl-level decorator keeps a single instance shared across every route it
/// wraps — for guards this is what gives e.g. `RateLimit` one bucket for the
/// whole controller. See [`ctrl_deco_set`] / [`generate_ctrl_deco_items`].
pub(crate) struct CtrlDecoSet {
    pub struct_ident: syn::Ident,
    pub ctor_ident: syn::Ident,
    /// Field idents for the controller-level interceptor sites (`__ci0..`),
    /// outermost-first in declaration order.
    pub intercept_fields: Vec<syn::Ident>,
    /// Field idents for the controller-level post-auth guard sites (`__cg0..`),
    /// roles-derived guards first — checked before each route's own guards.
    pub guard_fields: Vec<syn::Ident>,
    /// Field idents for the controller-level pre-auth guard sites (`__cp0..`),
    /// checked in the pre-auth middleware before each route's own pre-guards.
    pub pre_guard_fields: Vec<syn::Ident>,
}

/// Controller-level decorator exprs that actually apply, per family
/// (interceptors, post-auth guards, pre-auth guards).
///
/// Interceptors need an interceptable method (HTTP route, `#[scheduled]`, or
/// `#[consumer]`; SSE/WS do not run the interceptor chain) — without one the
/// family is dropped so no unused product gets built (its deps are not folded
/// either). Guards and pre-guards are parse-guaranteed to have an HTTP target
/// (`routes_parsing` rejects them otherwise), so they pass through as-is.
fn ctrl_family_exprs(def: &RoutesImplDef) -> (&[syn::Expr], &[syn::Expr], &[syn::Expr]) {
    let cd = &def.controller_decorators;
    let intercept_target = !def.route_methods.is_empty()
        || !def.scheduled_methods.is_empty()
        || !def.consumer_methods.is_empty();
    let intercepts: &[syn::Expr] = if intercept_target {
        &cd.intercept_fns
    } else {
        &[]
    };
    (intercepts, &cd.guard_fns, &cd.pre_auth_guard_fns)
}

/// Degradation check shared by every per-method emission site (handler +
/// closure, route/SSE/WS): true when the method-level exprs given AND every
/// applicable controller-level decorator expr resolve to a spec type. Handler
/// and closure MUST use the same check per method — a mismatch would change
/// one side's parameter arity but not the other's after a spec-type error.
pub(super) fn specs_ok_with_ctrl<'a>(
    def: &'a RoutesImplDef,
    method_exprs: impl IntoIterator<Item = &'a syn::Expr>,
) -> bool {
    let (intercepts, guards, pre_guards) = ctrl_family_exprs(def);
    all_specs_inferable(
        method_exprs
            .into_iter()
            .chain(intercepts)
            .chain(guards)
            .chain(pre_guards),
    )
}

/// Whether a controller has (inferable) controller-level decorator sites that
/// apply to at least one method. Returns the shared-set identifiers when so.
///
/// Deterministic from `def`, so every codegen site can recompute it without
/// threading the set through call signatures; the struct/ctor items themselves
/// are emitted once via [`generate_ctrl_deco_items`].
pub(crate) fn ctrl_deco_set(def: &RoutesImplDef) -> Option<CtrlDecoSet> {
    let (intercepts, guards, pre_guards) = ctrl_family_exprs(def);
    if intercepts.is_empty() && guards.is_empty() && pre_guards.is_empty() {
        return None;
    }
    if !all_specs_inferable(intercepts.iter().chain(guards).chain(pre_guards)) {
        return None;
    }
    let controller_name = &def.controller_name;
    Some(CtrlDecoSet {
        struct_ident: format_ident!("__R2eCtrlDeco_{}", controller_name),
        ctor_ident: format_ident!("__r2e_ctrldeco_{}", controller_name),
        intercept_fields: (0..intercepts.len())
            .map(|i| format_ident!("__ci{}", i))
            .collect(),
        guard_fields: (0..guards.len())
            .map(|i| format_ident!("__cg{}", i))
            .collect(),
        pre_guard_fields: (0..pre_guards.len())
            .map(|i| format_ident!("__cp{}", i))
            .collect(),
    })
}

/// Emit the shared controller-level decorator struct + its constructor
/// (built from the resolved bean context). Emitted **once** per controller at
/// module scope; the router body and the transverse fill each build a single
/// instance from it. Empty when there are no controller-level decorators.
pub(crate) fn generate_ctrl_deco_items(def: &RoutesImplDef) -> TokenStream {
    let (intercepts, guards, pre_guards) = ctrl_family_exprs(def);
    let Some(set) = ctrl_deco_set(def) else {
        // No shared set — either there is nothing to emit, or a non-inferable
        // controller-level spec gated it away. The latter MUST error here:
        // this is the only site that sees controller-level exprs (per-method
        // deco sets only receive method-level fns), so staying silent would
        // drop the whole controller-level chain — valid siblings included.
        let mut errors = TokenStream::new();
        for expr in intercepts.iter().chain(guards).chain(pre_guards) {
            if let Err(err) = spec_type_of(expr) {
                errors.extend(err.to_compile_error());
            }
        }
        return errors;
    };
    let sites = set
        .intercept_fields
        .iter()
        .zip(intercepts.iter())
        .chain(set.guard_fields.iter().zip(guards.iter()))
        .chain(set.pre_guard_fields.iter().zip(pre_guards.iter()));
    match emit_deco_struct(&set.struct_ident, &set.ctor_ident, sites, quote! {}) {
        Ok(items) => items,
        // Unreachable: `ctrl_deco_set` returned `Some`, so every spec is
        // inferable — kept as the error path for defense in depth.
        Err(err) => err,
    }
}

/// The `EndpointDeps::Deps` fold: the core's `ContextConstruct::Deps`
/// extended with every decorator site's `<Spec as DecoratorSpec>::Deps`,
/// deduplicated by spec type. All lists are concrete, so the `TAppend`
/// projections normalize without extra bounds on the impl.
///
/// Transport-neutral: `#[routes]` (via [`controller_deps_fold`]) and
/// `#[grpc_routes]` both fold their sites through here, so every endpoint
/// kind carries the same shape of dep list.
pub(crate) fn endpoint_deps_fold<'a>(
    endpoint_name: &syn::Ident,
    site_exprs: impl IntoIterator<Item = &'a syn::Expr>,
) -> TokenStream {
    let krate = r2e_core_path();
    deps_fold_from_base(
        quote! { <#endpoint_name as #krate::ContextConstruct>::Deps },
        site_exprs,
    )
}

/// Fold every decorator site's `<Spec as DecoratorSpec>::Deps` onto an
/// arbitrary `base` dep-list token stream, deduplicated by spec type. Used by
/// [`endpoint_deps_fold`] (base = `ContextConstruct::Deps`) and by `#[bean]`
/// (base = the constructor's `TCons` dep list) so bean decorator deps are
/// compile-checked at `.register::<T>()` alongside the constructor deps.
pub(crate) fn deps_fold_from_base<'a>(
    base: TokenStream,
    site_exprs: impl IntoIterator<Item = &'a syn::Expr>,
) -> TokenStream {
    let krate = r2e_core_path();

    let mut seen = std::collections::HashSet::new();
    let mut deps = base;
    for expr in site_exprs {
        if let Ok((spec, _)) = spec_type_of(expr) {
            if seen.insert(quote!(#spec).to_string()) {
                deps = quote! {
                    <#deps as #krate::type_list::TAppend<
                        <#spec as #krate::DecoratorSpec>::Deps,
                    >>::Output
                };
            }
        }
    }
    deps
}

/// The unique spec types of a set of decorator sites, in first-seen order.
/// Non-inferable expressions are skipped — `spec_type_of` already errors for
/// them wherever the site is actually built.
pub(crate) fn unique_spec_types<'a>(
    site_exprs: impl IntoIterator<Item = &'a syn::Expr>,
) -> Vec<syn::Path> {
    let mut seen = std::collections::HashSet::new();
    let mut specs = Vec::new();
    for expr in site_exprs {
        if let Ok((spec, _)) = spec_type_of(expr) {
            if seen.insert(quote!(#spec).to_string()) {
                specs.push(spec);
            }
        }
    }
    specs
}

/// Statements appending every decorator site's declared config keys' errors to
/// a `Vec<MissingKeyError>` binding named `__errors`.
///
/// This is where a `#[derive(DecoratorBean)]` guard/interceptor's `#[config]`
/// keys reach **aggregated** startup validation: the sites are known only to
/// the host (controller / bean), so the host reports them — under its own
/// registration banner, alongside its own missing keys — instead of the
/// decorator failing late inside `build_decorator`.
pub(crate) fn decorator_config_key_stmts<'a>(
    site_exprs: impl IntoIterator<Item = &'a syn::Expr>,
) -> Vec<TokenStream> {
    let krate = r2e_core_path();
    unique_spec_types(site_exprs)
        .into_iter()
        .map(|spec| {
            quote! {
                __errors.extend(#krate::decorator_config_errors::<#spec>(__config));
            }
        })
        .collect()
}

/// Every decorator site expression of a `#[routes]` block, in the order the
/// dep fold visits them. Shared by [`controller_deps_fold`] and the
/// config-key aggregation so both see exactly the same site set.
pub(super) fn controller_site_exprs(def: &RoutesImplDef) -> Vec<&syn::Expr> {
    let mut exprs: Vec<&syn::Expr> = Vec::new();

    // Controller-level sites, gated per family on an applicable target
    // (interceptors need an interceptable method; guards/pre-guards are
    // parse-guaranteed one) — exactly the exprs the shared set builds.
    let (ctrl_intercepts, ctrl_guards, ctrl_pre_guards) = ctrl_family_exprs(def);
    exprs.extend(ctrl_intercepts);
    exprs.extend(ctrl_guards);
    exprs.extend(ctrl_pre_guards);
    for rm in &def.route_methods {
        exprs.extend(&rm.decorators.guard_fns);
        exprs.extend(&rm.decorators.pre_auth_guard_fns);
        exprs.extend(&rm.decorators.intercept_fns);
    }
    // Scheduled + consumer methods run interceptors (built once at
    // registration, from the retained context, via `fill_decos`).
    for sm in &def.scheduled_methods {
        exprs.extend(&sm.intercept_fns);
    }
    for cm in &def.consumer_methods {
        exprs.extend(&cm.intercept_fns);
    }
    // SSE/WS methods run guards (and pre-auth guards) but not interceptors.
    for sm in &def.sse_methods {
        exprs.extend(&sm.decorators.guard_fns);
        exprs.extend(&sm.decorators.pre_auth_guard_fns);
    }
    for wm in &def.ws_methods {
        exprs.extend(&wm.decorators.guard_fns);
        exprs.extend(&wm.decorators.pre_auth_guard_fns);
    }

    exprs
}

/// [`endpoint_deps_fold`] over every decorator site of a `#[routes]` block,
/// plus the [`ManagedDeps`] of every distinct `#[managed]` parameter type.
///
/// Managed resources resolve their collaborators dynamically inside `acquire`
/// (`state.bean::<T>()`), which cannot fail at compile time — folding
/// `<Ty as ManagedDeps>::Deps` in here is what turns "pool was never provided"
/// from a runtime 500 into a `register_controller` compile error.
pub(super) fn controller_deps_fold(def: &RoutesImplDef) -> TokenStream {
    let krate = r2e_core_path();
    let mut deps = endpoint_deps_fold(&def.controller_name, controller_site_exprs(def));
    let mut seen = std::collections::HashSet::new();
    for rm in &def.route_methods {
        for mp in &rm.managed_params {
            let ty = crate::util::type_utils::staticize_lifetimes(&mp.ty);
            if seen.insert(quote!(#ty).to_string()) {
                deps = quote! {
                    <#deps as #krate::type_list::TAppend<
                        <#ty as #krate::ManagedDeps>::Deps,
                    >>::Output
                };
            }
        }
    }
    deps
}
