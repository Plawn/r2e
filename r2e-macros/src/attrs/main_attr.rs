//! `#[r2e::main]` and `#[r2e::test]` attribute macros.
//!
//! These wrap the user's `async fn main()` / `async fn test_*()` in a Tokio
//! runtime and optionally call `init_tracing()`.
//!
//! The canonical way to declare an app is `impl r2e::App` + a parameterless
//! `#[r2e::main]` that calls [`r2e::launch`](r2e_core::launch):
//!
//! ```ignore
//! #[r2e::main]
//! async fn main() {
//!     r2e::launch::<MyApp>().await.unwrap();
//! }
//! ```
//!
//! Dev-mode hot-reload is handled entirely inside `launch` (behind the
//! `dev-reload` feature); the macro no longer generates any hot-reload paths.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{parse_macro_input, FnArg, ItemFn};

use crate::util::crate_path::r2e_core_path;
use crate::util::runtime_args::{parse_bool, type_ends_with, KeyedExpr, KeyedFlag, RuntimeArgs};

// ── Argument parsing ─────────────────────────────────────────────────────

struct MainArgs {
    tracing: bool,
    /// `#[r2e::test(app = ...)]`: the `App`-implementing type to boot into a
    /// `TestApp`.
    app_fn: Option<syn::Path>,
    /// `#[r2e::test(app = ..., with = |b| ...)]`: builder pre-configuration hook.
    with_expr: Option<KeyedExpr>,
    /// `#[r2e::test(app = ..., env = expr)]`: boot on an already-built
    /// `App::Env` instead of calling `App::setup()`. The expression is
    /// evaluated inside the test's async block (so `env = ENV.get().await` is
    /// fine) and must produce `<App as App>::Env`.
    env_expr: Option<KeyedExpr>,
    /// `#[r2e::test(app = ..., jwt = false)]`: skip the TestJwt auto-wiring.
    jwt: bool,
    /// Where `jwt = ...` was written, for the diagnostic that rejects it
    /// without `app = ...`.
    jwt_key: Option<KeyedFlag>,
    /// `#[r2e::test(order = <u32>)]`: run this test sequentially (ascending
    /// `order`) via the r2e-test static barrier. Test-only. The literal is kept
    /// for span-accurate error reporting; the value is parsed as `u32`.
    order: Option<syn::LitInt>,
    /// `#[r2e::test(order = ..., group = "<str>")]`: barrier group name. Only
    /// meaningful together with `order`. Defaults to `""` when omitted.
    group: Option<syn::LitStr>,
    /// Shared Tokio `runtime::Builder` knobs (`flavor`, `worker_threads`, …).
    runtime: RuntimeArgs,
}

impl Default for MainArgs {
    fn default() -> Self {
        Self {
            tracing: true,
            app_fn: None,
            with_expr: None,
            env_expr: None,
            jwt: true,
            jwt_key: None,
            order: None,
            group: None,
            runtime: RuntimeArgs::default(),
        }
    }
}

impl MainArgs {
    fn parse(args: TokenStream) -> syn::Result<Self> {
        let mut this = Self::default();

        if args.is_empty() {
            return Ok(this);
        }

        let parser = syn::meta::parser(|meta| {
            let key = meta
                .path
                .get_ident()
                .map(|i| i.to_string())
                .unwrap_or_default();

            if meta.input.peek(syn::Token![=]) {
                match key.as_str() {
                    "tracing" => this.tracing = parse_bool(&meta)?,
                    "app" => this.app_fn = Some(meta.value()?.parse()?),
                    "with" => this.with_expr = Some(KeyedExpr::parse(&meta)?),
                    "env" => this.env_expr = Some(KeyedExpr::parse(&meta)?),
                    "jwt" => {
                        this.jwt_key = Some(KeyedFlag {
                            key: meta.path.clone(),
                        });
                        this.jwt = parse_bool(&meta)?;
                    }
                    "order" => this.order = Some(meta.value()?.parse()?),
                    "group" => this.group = Some(meta.value()?.parse()?),
                    _ => {
                        if !this.runtime.try_parse(&key, &meta)? {
                            return Err(meta.error(format!("unknown argument `{key}`")));
                        }
                    }
                }
            } else {
                return Err(meta.error(format!(
                    "unexpected argument `{key}` — `#[r2e::main]` takes no bare-path \
                     arguments (the old `setup` hot-reload convention was removed; \
                     declare your app via `impl r2e::App` and call \
                     `r2e::launch::<MyApp>()`)"
                )));
            }
            Ok(())
        });

        syn::parse::Parser::parse(parser, args)?;
        // Knob combinations the builder panics on are compile errors here, not
        // an anonymous tokio panic at test time.
        this.runtime.validate()?;
        Ok(this)
    }

    /// The arguments that only mean something on a boot (`app = ...`), spanned
    /// where the user wrote them. `None` when there is nothing to reject.
    fn reject_app_only_args(&self) -> Option<syn::Error> {
        if let Some(env) = &self.env_expr {
            return Some(syn::Error::new_spanned(
                env,
                "`env = ...` requires `app = <App type>` — the environment is handed to \
                 `App::build`, so there is nothing to boot it into:\n\n\
                 \x20 #[r2e::test(app = MyApp, env = ENV.get().await)]",
            ));
        }
        if let Some(with) = &self.with_expr {
            return Some(syn::Error::new_spanned(
                with,
                "`with = ...` requires `app = <App type>` — the hook pre-configures the \
                 `AppBuilder` of the app being booted",
            ));
        }
        if let Some(jwt) = &self.jwt_key {
            return Some(syn::Error::new_spanned(
                jwt,
                "`jwt = ...` requires `app = <App type>` — it turns the harness's `TestJwt` \
                 wiring off on the booted app",
            ));
        }
        None
    }
}

// ── Codegen ──────────────────────────────────────────────────────────────

fn expand_inner(args: MainArgs, func: ItemFn, is_test: bool) -> TokenStream2 {
    let krate = r2e_core_path();
    let vis = &func.vis;
    let sig = &func.sig;
    let attrs = &func.attrs;
    let body = &func.block;
    let fn_name = &sig.ident;
    let ret = &sig.output;

    // Validate: function must be async
    if sig.asyncness.is_none() {
        return syn::Error::new_spanned(
            sig.fn_token,
            if is_test {
                "#[r2e::test] requires an async function"
            } else {
                "#[r2e::main] requires an async function"
            },
        )
        .to_compile_error();
    }

    // ── `order` / `group` validation (test-only ordering barrier) ────────
    // `order` and `group` are only accepted on `#[r2e::test]`.
    if !is_test {
        if let Some(order_lit) = &args.order {
            return syn::Error::new_spanned(
                order_lit,
                "`order` is only supported on `#[r2e::test]`",
            )
            .to_compile_error();
        }
        if let Some(group_lit) = &args.group {
            return syn::Error::new_spanned(
                group_lit,
                "`group` is only supported on `#[r2e::test]`",
            )
            .to_compile_error();
        }
    }
    // `group` only groups tests that also declare an `order`.
    if let Some(group_lit) = &args.group {
        if args.order.is_none() {
            return syn::Error::new_spanned(
                group_lit,
                "`group` requires `order` — `group` only names the barrier for tests that also declare an `order`",
            )
            .to_compile_error();
        }
    }

    // Resolve the ordering hooks once (only when `order` is present). Parsing
    // the literal as `u32` here surfaces negative / non-integer literals as an
    // error spanned on the literal.
    let ordering: Option<OrderedHooks> = match &args.order {
        Some(order_lit) => {
            let order_val: u32 = match order_lit.base10_parse() {
                Ok(v) => v,
                Err(e) => return e.to_compile_error(),
            };
            let group_lit = match &args.group {
                Some(g) => quote! { #g },
                None => quote! { "" },
            };
            Some(OrderedHooks::new(fn_name, order_val, &group_lit, attrs))
        }
        None => None,
    };

    let tracing_init = if args.tracing {
        quote! { #krate::init_tracing(); }
    } else {
        quote! {}
    };

    let runtime_builder = args.runtime.builder_tokens(&krate);

    let test_attr = if is_test {
        quote! { #[::core::prelude::v1::test] }
    } else {
        quote! {}
    };

    // ── App-boot path: #[r2e::test(app = MyApp)] ─────────────────────────
    if args.app_fn.is_some() && !is_test {
        return syn::Error::new_spanned(sig, "`app = ...` is only valid on #[r2e::test]")
            .to_compile_error();
    }
    if args.app_fn.is_none() {
        // Spanned on the argument itself, not on the item: the user's mistake
        // is the argument they wrote, and `env = <expr>` in particular is
        // usually a `#[r2e::test]` that lost its `app = ...`.
        if let Some(err) = args.reject_app_only_args() {
            return err.to_compile_error();
        }
    }
    if let Some(app_ty) = &args.app_fn {
        return expand_boot_test(
            app_ty,
            args.with_expr.as_ref().map(|k| &k.expr),
            args.env_expr.as_ref().map(|k| &k.expr),
            args.jwt,
            ordering.as_ref(),
            &func,
            &tracing_init,
            &runtime_builder,
        );
    }

    // A parameter on `main` used to trigger the hot-reload `setup` convention;
    // that path is gone. Point users at the `App` + `launch` pattern.
    if !is_test && sig.inputs.first().is_some() {
        return syn::Error::new_spanned(
            sig,
            "#[r2e::main] does not accept parameters — declare your app via \
             `impl r2e::App` and call `r2e::launch::<MyApp>()` in a parameterless \
             main:\n\n    #[r2e::main]\n    async fn main() { \
             r2e::launch::<MyApp>().await.unwrap() }",
        )
        .to_compile_error();
    }

    // The standard path rebuilds the function as `fn #fn_name()`, so a
    // parameter would be silently discarded. Only the `app = ...` path knows how
    // to bind one (task #985).
    if is_test && sig.inputs.first().is_some() {
        return syn::Error::new_spanned(
            sig,
            "#[r2e::test] does not accept parameters unless it boots an app — \
             parameters are bound from the booted `TestApp`:\n\n\
             \x20 #[r2e::test(app = MyApp)]\n\
             \x20 async fn t(app: TestApp) { ... }",
        )
        .to_compile_error();
    }

    // ── Standard path ─────────────────────────────────────────────────────
    // Ordered tests (test-only) enroll in the r2e-test sequential barrier: an
    // inventory entry at item level, the turn guard as first statement, and
    // the body wrapped so its outcome reaches the guard (an `Err` from a
    // `Result` test poisons the group like a panic). For unordered tests
    // `#submit` renders to nothing and the body is emitted untouched (no
    // reference to r2e-test).
    let body_stmts = &body.stmts;
    let (submit, async_body) = match &ordering {
        Some(hooks) => {
            let turn = &hooks.turn;
            let wrapped = hooks.wrap_body(&quote! { #(#body_stmts)* });
            (hooks.submit.clone(), quote! { #turn #wrapped })
        }
        None => (quote! {}, quote! { #(#body_stmts)* }),
    };

    quote! {
        #submit
        #(#attrs)*
        #test_attr
        #vis fn #fn_name() #ret {
            #tracing_init
            #runtime_builder
                .block_on(async { #async_body })
        }
    }
}

/// Emission hooks for an ordered test (`order = …`):
/// - `submit` — the item-level `inventory::submit!` registering the test's
///   `OrderedTestEntry` in the binary-wide registry;
/// - `turn` — the first statement(s) of the async block: acquire the barrier
///   guard (synchronous, so it also covers app boot) and, for
///   `#[should_panic]` tests, declare the panic expected so it does not
///   poison the group;
/// - [`Self::wrap_body`] — wraps the user body so its outcome reaches the
///   guard (an `Err` from a `Result` test poisons the group).
struct OrderedHooks {
    submit: TokenStream2,
    turn: TokenStream2,
    test_crate: TokenStream2,
}

impl OrderedHooks {
    fn new(
        fn_name: &syn::Ident,
        order_val: u32,
        group_lit: &TokenStream2,
        attrs: &[syn::Attribute],
    ) -> Self {
        let test_crate = crate::util::crate_path::r2e_test_path();
        let submit = quote! {
            #test_crate::ordering::inventory::submit! {
                #test_crate::ordering::OrderedTestEntry {
                    group: #group_lit,
                    order: #order_val,
                    test: concat!(module_path!(), "::", stringify!(#fn_name)),
                }
            }
        };
        // `#[should_panic]`: the panic IS the test's success path — it must
        // not poison the group.
        let expect_panic = attrs
            .iter()
            .any(|a| a.path().is_ident("should_panic"))
            .then(|| quote! { __r2e_ordered_turn.expect_panic(); });
        let turn = quote! {
            let mut __r2e_ordered_turn = #test_crate::ordering::turn(
                #group_lit,
                #order_val,
                concat!(module_path!(), "::", stringify!(#fn_name)),
            );
            #expect_panic
        };
        Self {
            submit,
            turn,
            test_crate,
        }
    }

    /// Wrap the user body so its outcome reaches the guard before it drops:
    /// an `Err` from a `Result` test marks the order failed (group poison).
    fn wrap_body(&self, stmts: &TokenStream2) -> TokenStream2 {
        let test_crate = &self.test_crate;
        quote! {
            let __r2e_ordered_outcome = async { #stmts }.await;
            if #test_crate::ordering::TestOutcome::is_failed(&__r2e_ordered_outcome) {
                __r2e_ordered_turn.mark_failed();
            }
            __r2e_ordered_outcome
        }
    }
}

/// Codegen for `#[r2e::test(app = <MyApp>)]`: boots the `App`-implementing
/// type into a `TestApp` and binds the test function's parameters from it.
///
/// Parameter forms:
/// - `app: TestApp` — the booted app (at most one),
/// - `jwt: TestJwt` — a clone of the app's auto-wired `TestJwt`,
/// - `#[inject] bean: T` — `app.bean::<T>()` from the resolved graph.
// One parameter per knob the attribute accepts; bundling them into a struct
// would only move the same list one indirection away.
#[allow(clippy::too_many_arguments)]
fn expand_boot_test(
    app_ty: &syn::Path,
    with_expr: Option<&syn::Expr>,
    env_expr: Option<&syn::Expr>,
    jwt: bool,
    ordering: Option<&OrderedHooks>,
    func: &ItemFn,
    tracing_init: &TokenStream2,
    runtime_builder: &TokenStream2,
) -> TokenStream2 {
    let test_crate = crate::util::crate_path::r2e_test_path();
    let vis = &func.vis;
    let sig = &func.sig;
    let attrs = &func.attrs;
    let body_stmts = &func.block.stmts;
    let fn_name = &sig.ident;
    let ret = &sig.output;

    let configure: TokenStream2 = match with_expr {
        Some(expr) => quote! { #expr },
        None => quote! { |__r2e_b| __r2e_b },
    };
    // `env = expr` boots on an environment the caller already owns (a
    // `LazyLock`/`OnceCell` shared by the whole test binary), so `App::setup()`
    // is not called again.
    let boot_call = match (jwt, env_expr) {
        (true, None) => quote! { #test_crate::TestApp::boot_with::<#app_ty>(#configure).await },
        (false, None) => quote! { #test_crate::TestApp::boot_plain::<#app_ty>(#configure).await },
        (true, Some(env)) => {
            quote! { #test_crate::TestApp::boot_with_env::<#app_ty>(#env, #configure).await }
        }
        (false, Some(env)) => {
            quote! { #test_crate::TestApp::boot_plain_env::<#app_ty>(#env, #configure).await }
        }
    };

    // Bind parameters from the booted app. The `TestApp` binding moves the
    // app, so it is emitted last.
    let mut bindings: Vec<TokenStream2> = Vec::new();
    let mut app_binding: Option<TokenStream2> = None;
    for input in &sig.inputs {
        let param = match input {
            FnArg::Typed(param) => param,
            FnArg::Receiver(recv) => {
                return syn::Error::new_spanned(
                    recv,
                    "#[r2e::test(app = ...)] does not support `self` parameters",
                )
                .to_compile_error();
            }
        };
        let pat = &param.pat;
        let ty = &param.ty;
        let is_inject = param.attrs.iter().any(|a| a.path().is_ident("inject"));

        if is_inject {
            bindings.push(quote! { let #pat: #ty = __r2e_test_app.bean::<#ty>(); });
        } else if type_ends_with(ty, "TestApp") {
            if app_binding.is_some() {
                return syn::Error::new_spanned(param, "only one `TestApp` parameter is allowed")
                    .to_compile_error();
            }
            app_binding = Some(quote! { let #pat: #ty = __r2e_test_app; });
        } else if type_ends_with(ty, "TestJwt") {
            bindings.push(quote! { let #pat: #ty = __r2e_test_app.test_jwt().clone(); });
        } else {
            return syn::Error::new_spanned(
                param,
                "parameters of a blueprint test must be `TestApp`, `TestJwt`, \
                 or a bean marked `#[inject]` (e.g. `#[inject] service: UserService`)",
            )
            .to_compile_error();
        }
    }
    let app_binding = app_binding.into_iter();

    // Ordering barrier hooks (test-only). Empty when `order` is absent, so the
    // unordered expansion is unchanged. The `turn` guard must precede app boot,
    // and the body is wrapped so an `Err` outcome poisons the group.
    let (submit, turn, wrapped_body) = match ordering {
        Some(hooks) => (
            hooks.submit.clone(),
            hooks.turn.clone(),
            hooks.wrap_body(&quote! { #(#body_stmts)* }),
        ),
        None => (quote! {}, quote! {}, quote! { #(#body_stmts)* }),
    };

    quote! {
        #submit
        #(#attrs)*
        #[::core::prelude::v1::test]
        #vis fn #fn_name() #ret {
            #tracing_init
            #runtime_builder
                .block_on(async {
                    #turn
                    let __r2e_test_app = #boot_call;
                    #(#bindings)*
                    #(#app_binding)*
                    #wrapped_body
                })
        }
    }
}

pub fn expand_main(args: TokenStream, input: TokenStream) -> TokenStream {
    let func = parse_macro_input!(input as ItemFn);
    let parsed_args = match MainArgs::parse(args) {
        Ok(a) => a,
        Err(e) => return e.to_compile_error().into(),
    };
    expand_inner(parsed_args, func, false).into()
}

pub fn expand_test(args: TokenStream, input: TokenStream) -> TokenStream {
    let func = parse_macro_input!(input as ItemFn);
    let parsed_args = match MainArgs::parse(args) {
        Ok(a) => a,
        Err(e) => return e.to_compile_error().into(),
    };
    expand_inner(parsed_args, func, true).into()
}
