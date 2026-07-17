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

use crate::crate_path::r2e_core_path;

// ── Argument parsing ─────────────────────────────────────────────────────

/// `None` = user did not set `flavor`, `Some(true)` = current_thread, `Some(false)` = multi_thread
struct MainArgs {
    tracing: bool,
    worker_threads: Option<usize>,
    flavor: Option<bool>,
    /// `#[r2e::test(app = ...)]`: the `App`-implementing type to boot into a
    /// `TestApp`.
    app_fn: Option<syn::Path>,
    /// `#[r2e::test(app = ..., with = |b| ...)]`: builder pre-configuration hook.
    with_expr: Option<syn::Expr>,
    /// `#[r2e::test(app = ..., jwt = false)]`: skip the TestJwt auto-wiring.
    jwt: bool,
    /// `#[r2e::test(order = <u32>)]`: run this test sequentially (ascending
    /// `order`) via the r2e-test static barrier. Test-only. The literal is kept
    /// for span-accurate error reporting; the value is parsed as `u32`.
    order: Option<syn::LitInt>,
    /// `#[r2e::test(order = ..., group = "<str>")]`: barrier group name. Only
    /// meaningful together with `order`. Defaults to `""` when omitted.
    group: Option<syn::LitStr>,
    max_blocking_threads: Option<usize>,
    thread_stack_size: Option<usize>,
    thread_name: Option<String>,
    global_queue_interval: Option<u32>,
    event_interval: Option<u32>,
    thread_keep_alive_secs: Option<u64>,
    start_paused: Option<bool>,
}

impl Default for MainArgs {
    fn default() -> Self {
        Self {
            tracing: true,
            worker_threads: None,
            flavor: None,
            app_fn: None,
            with_expr: None,
            jwt: true,
            order: None,
            group: None,
            max_blocking_threads: None,
            thread_stack_size: None,
            thread_name: None,
            global_queue_interval: None,
            event_interval: None,
            thread_keep_alive_secs: None,
            start_paused: None,
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
                    "flavor" => {
                        let s: syn::LitStr = meta.value()?.parse()?;
                        this.flavor = Some(match s.value().as_str() {
                            "current_thread" => true,
                            "multi_thread" => false,
                            other => {
                                return Err(syn::Error::new_spanned(
                                    &s,
                                    format!(
                                        "unknown flavor \"{other}\", expected \"current_thread\" or \"multi_thread\""
                                    ),
                                ));
                            }
                        });
                    }
                    "app" => this.app_fn = Some(meta.value()?.parse()?),
                    "with" => this.with_expr = Some(meta.value()?.parse()?),
                    "jwt" => this.jwt = parse_bool(&meta)?,
                    "order" => this.order = Some(meta.value()?.parse()?),
                    "group" => this.group = Some(meta.value()?.parse()?),
                    "worker_threads" => this.worker_threads = Some(parse_int(&meta)?),
                    "max_blocking_threads" => this.max_blocking_threads = Some(parse_int(&meta)?),
                    "thread_stack_size" => this.thread_stack_size = Some(parse_int(&meta)?),
                    "thread_name" => this.thread_name = Some(parse_str(&meta)?),
                    "global_queue_interval" => this.global_queue_interval = Some(parse_int(&meta)?),
                    "event_interval" => this.event_interval = Some(parse_int(&meta)?),
                    "thread_keep_alive" => this.thread_keep_alive_secs = Some(parse_int(&meta)?),
                    "start_paused" => this.start_paused = Some(parse_bool(&meta)?),
                    _ => return Err(meta.error(format!("unknown argument `{key}`"))),
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
        Ok(this)
    }

    /// Whether to use `new_current_thread()`. Defaults to multi_thread for
    /// both main and tests unless explicitly overridden.
    fn use_current_thread(&self, is_test: bool) -> bool {
        match self.flavor {
            Some(current) => current,
            None => {
                let _ = is_test;
                false
            }
        }
    }

    /// Generate the full `tokio::runtime::Builder` chain including `.build()`.
    fn runtime_builder_tokens(&self, is_test: bool) -> TokenStream2 {
        let builder_fn = if self.use_current_thread(is_test) {
            quote! { ::tokio::runtime::Builder::new_current_thread() }
        } else {
            quote! { ::tokio::runtime::Builder::new_multi_thread() }
        };

        let worker_threads = self.worker_threads.map(|n| {
            quote! { .worker_threads(#n) }
        });
        let max_blocking = self.max_blocking_threads.map(|n| {
            quote! { .max_blocking_threads(#n) }
        });
        let stack_size = self.thread_stack_size.map(|n| {
            quote! { .thread_stack_size(#n) }
        });
        let thread_name = self.thread_name.as_ref().map(|s| {
            quote! { .thread_name(#s) }
        });
        let gqi = self.global_queue_interval.map(|n| {
            quote! { .global_queue_interval(#n) }
        });
        let ei = self.event_interval.map(|n| {
            quote! { .event_interval(#n) }
        });
        let keep_alive = self.thread_keep_alive_secs.map(|secs| {
            quote! { .thread_keep_alive(::std::time::Duration::from_secs(#secs)) }
        });
        let start_paused = self.start_paused.map(|b| {
            quote! { .start_paused(#b) }
        });

        quote! {
            #builder_fn
                #worker_threads
                #max_blocking
                #stack_size
                #thread_name
                #gqi
                #ei
                #keep_alive
                #start_paused
                .enable_all()
                .build()
                .expect("failed to build tokio runtime")
        }
    }
}

fn parse_bool(meta: &syn::meta::ParseNestedMeta) -> syn::Result<bool> {
    let b: syn::LitBool = meta.value()?.parse()?;
    Ok(b.value)
}

fn parse_int<T: std::str::FromStr>(meta: &syn::meta::ParseNestedMeta) -> syn::Result<T>
where
    T::Err: std::fmt::Display,
{
    let i: syn::LitInt = meta.value()?.parse()?;
    i.base10_parse()
}

fn parse_str(meta: &syn::meta::ParseNestedMeta) -> syn::Result<String> {
    let s: syn::LitStr = meta.value()?.parse()?;
    Ok(s.value())
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
    let ordering: Option<(u32, TokenStream2)> = match &args.order {
        Some(order_lit) => {
            let order_val: u32 = match order_lit.base10_parse() {
                Ok(v) => v,
                Err(e) => return e.to_compile_error(),
            };
            let group_lit = match &args.group {
                Some(g) => quote! { #g },
                None => quote! { "" },
            };
            Some((order_val, group_lit))
        }
        None => None,
    };

    let tracing_init = if args.tracing {
        quote! { #krate::init_tracing(); }
    } else {
        quote! {}
    };

    let runtime_builder = args.runtime_builder_tokens(is_test);

    let test_attr = if is_test {
        quote! { #[::core::prelude::v1::test] }
    } else {
        quote! {}
    };

    // ── App-boot path: #[r2e::test(app = MyApp)] ─────────────────────────
    if args.app_fn.is_some() && !is_test {
        return syn::Error::new_spanned(
            sig,
            "`app = ...` is only valid on #[r2e::test]",
        )
        .to_compile_error();
    }
    if args.app_fn.is_none() && (args.with_expr.is_some() || !args.jwt) {
        return syn::Error::new_spanned(
            sig,
            "`with = ...` and `jwt = ...` require `app = <App type>`",
        )
        .to_compile_error();
    }
    if let Some(app_ty) = &args.app_fn {
        return expand_boot_test(
            app_ty,
            args.with_expr.as_ref(),
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

    // ── Standard path ─────────────────────────────────────────────────────
    // Ordered tests (test-only) enroll in the r2e-test sequential barrier: an
    // inventory entry at item level and a `turn` guard held for the whole async
    // block. Unordered tests keep the byte-identical expansion of before (no
    // reference to r2e-test).
    if let Some((order_val, group_lit)) = &ordering {
        let test_crate = crate::crate_path::r2e_test_path();
        let (submit, turn) = ordering_tokens(&test_crate, fn_name, *order_val, group_lit);
        let body_stmts = &body.stmts;
        return quote! {
            #submit
            #(#attrs)*
            #test_attr
            #vis fn #fn_name() #ret {
                #tracing_init
                #runtime_builder
                    .block_on(async {
                        #turn
                        #(#body_stmts)*
                    })
            }
        };
    }

    quote! {
        #(#attrs)*
        #test_attr
        #vis fn #fn_name() #ret {
            #tracing_init
            #runtime_builder
                .block_on(async #body)
        }
    }
}

/// Build the two token streams that wire a test into the r2e-test sequential
/// barrier when `order` is set:
/// - an item-level `inventory::submit!` registering the test's `OrderedTestEntry`,
/// - the first statement of the async block: a `turn(...)` guard (`#[must_use]`,
///   RAII) that must stay alive until the end of the block (covers app boot).
fn ordering_tokens(
    test_crate: &TokenStream2,
    fn_name: &syn::Ident,
    order_val: u32,
    group_lit: &TokenStream2,
) -> (TokenStream2, TokenStream2) {
    let submit = quote! {
        #test_crate::ordering::inventory::submit! {
            #test_crate::ordering::OrderedTestEntry {
                group: #group_lit,
                order: #order_val,
                test: concat!(module_path!(), "::", stringify!(#fn_name)),
            }
        }
    };
    let turn = quote! {
        let __r2e_ordered_turn = #test_crate::ordering::turn(
            #group_lit,
            #order_val,
            concat!(module_path!(), "::", stringify!(#fn_name)),
        )
        .await;
    };
    (submit, turn)
}

/// Returns `true` if `ty` is a path type whose last segment is `name`
/// (matches both `TestApp` and `r2e_test::TestApp`).
fn type_ends_with(ty: &syn::Type, name: &str) -> bool {
    match ty {
        syn::Type::Path(p) => p
            .path
            .segments
            .last()
            .map(|seg| seg.ident == name)
            .unwrap_or(false),
        _ => false,
    }
}

/// Codegen for `#[r2e::test(app = <MyApp>)]`: boots the `App`-implementing
/// type into a `TestApp` and binds the test function's parameters from it.
///
/// Parameter forms:
/// - `app: TestApp` — the booted app (at most one),
/// - `jwt: TestJwt` — a clone of the app's auto-wired `TestJwt`,
/// - `#[inject] bean: T` — `app.bean::<T>()` from the resolved graph.
fn expand_boot_test(
    app_ty: &syn::Path,
    with_expr: Option<&syn::Expr>,
    jwt: bool,
    ordering: Option<&(u32, TokenStream2)>,
    func: &ItemFn,
    tracing_init: &TokenStream2,
    runtime_builder: &TokenStream2,
) -> TokenStream2 {
    let test_crate = crate::crate_path::r2e_test_path();
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
    let boot_call = if jwt {
        quote! { #test_crate::TestApp::boot_with::<#app_ty>(#configure).await }
    } else {
        quote! { #test_crate::TestApp::boot_plain::<#app_ty>(#configure).await }
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
                return syn::Error::new_spanned(
                    param,
                    "only one `TestApp` parameter is allowed",
                )
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
    // unordered expansion is unchanged. The `turn` guard must precede app boot.
    let (submit, turn) = match ordering {
        Some((order_val, group_lit)) => {
            ordering_tokens(&test_crate, fn_name, *order_val, group_lit)
        }
        None => (quote! {}, quote! {}),
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
                    #(#body_stmts)*
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
