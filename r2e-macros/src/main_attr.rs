//! `#[r2e::main]` and `#[r2e::test]` attribute macros.
//!
//! These wrap the user's `async fn main()` / `async fn test_*()` in a Tokio
//! runtime and optionally call `init_tracing()`.
//!
//! # Hot-reload support
//!
//! When a setup function is specified, the macro generates two code paths
//! gated by `#[cfg(feature = "dev-reload")]`:
//!
//! ```ignore
//! #[r2e::main(setup)]
//! async fn main(env: AppEnv) {
//!     // ... body is hot-patched when dev-reload is enabled
//! }
//! ```

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{parse_macro_input, Expr, ExprLit, FnArg, ItemFn, Lit, Meta};

use crate::crate_path::{r2e_core_path, r2e_devtools_path};

// ── Argument parsing ─────────────────────────────────────────────────────

/// `None` = user did not set `flavor`, `Some(true)` = current_thread, `Some(false)` = multi_thread
struct MainArgs {
    tracing: bool,
    worker_threads: Option<usize>,
    flavor: Option<bool>,
    /// Optional setup function path for hot-reload support.
    setup_fn: Option<syn::Path>,
}

impl Default for MainArgs {
    fn default() -> Self {
        Self {
            tracing: true,
            worker_threads: None,
            flavor: None,
            setup_fn: None,
        }
    }
}

impl MainArgs {
    fn parse(args: TokenStream) -> syn::Result<Self> {
        let mut this = Self::default();

        if args.is_empty() {
            return Ok(this);
        }

        let meta_list: syn::punctuated::Punctuated<Meta, syn::Token![,]> =
            syn::parse::Parser::parse(
                syn::punctuated::Punctuated::parse_terminated,
                args,
            )?;

        for meta in meta_list {
            match &meta {
                Meta::NameValue(nv) => {
                    let key = nv
                        .path
                        .get_ident()
                        .map(|i| i.to_string())
                        .unwrap_or_default();

                    match key.as_str() {
                        "tracing" => {
                            if let Expr::Lit(ExprLit {
                                lit: Lit::Bool(b), ..
                            }) = &nv.value
                            {
                                this.tracing = b.value;
                            } else {
                                return Err(syn::Error::new_spanned(
                                    &nv.value,
                                    "expected a boolean literal for `tracing`",
                                ));
                            }
                        }
                        "flavor" => {
                            if let Expr::Lit(ExprLit {
                                lit: Lit::Str(s), ..
                            }) = &nv.value
                            {
                                match s.value().as_str() {
                                    "current_thread" => this.flavor = Some(true),
                                    "multi_thread" => this.flavor = Some(false),
                                    other => {
                                        return Err(syn::Error::new_spanned(
                                            s,
                                            format!(
                                                "unknown flavor \"{other}\", expected \"current_thread\" or \"multi_thread\""
                                            ),
                                        ));
                                    }
                                }
                            } else {
                                return Err(syn::Error::new_spanned(
                                    &nv.value,
                                    "expected a string literal for `flavor`",
                                ));
                            }
                        }
                        "worker_threads" => {
                            if let Expr::Lit(ExprLit {
                                lit: Lit::Int(i), ..
                            }) = &nv.value
                            {
                                this.worker_threads = Some(i.base10_parse()?);
                            } else {
                                return Err(syn::Error::new_spanned(
                                    &nv.value,
                                    "expected an integer literal for `worker_threads`",
                                ));
                            }
                        }
                        _ => {
                            return Err(syn::Error::new_spanned(
                                &nv.path,
                                format!("unknown argument `{key}`"),
                            ));
                        }
                    }
                }
                // Bare path (e.g. `setup`) → setup function name
                Meta::Path(path) => {
                    if this.setup_fn.is_some() {
                        return Err(syn::Error::new_spanned(
                            path,
                            "setup function already specified",
                        ));
                    }
                    this.setup_fn = Some(path.clone());
                }
                other => {
                    return Err(syn::Error::new_spanned(
                        other,
                        "expected a setup function name or `key = value` arguments",
                    ));
                }
            }
        }

        Ok(this)
    }

    /// Whether to use `new_current_thread()`. Tests default to current_thread,
    /// main defaults to multi_thread.
    fn use_current_thread(&self, is_test: bool) -> bool {
        match self.flavor {
            Some(current) => current,
            None => is_test, // tests → current_thread, main → multi_thread
        }
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

    let tracing_init = if args.tracing {
        quote! { #krate::init_tracing(); }
    } else {
        quote! {}
    };

    let builder_fn = if args.use_current_thread(is_test) {
        quote! { ::tokio::runtime::Builder::new_current_thread() }
    } else {
        quote! { ::tokio::runtime::Builder::new_multi_thread() }
    };

    let worker_threads = args.worker_threads.map(|n| {
        quote! { .worker_threads(#n) }
    });

    let test_attr = if is_test {
        quote! { #[::core::prelude::v1::test] }
    } else {
        quote! {}
    };

    // ── Hot-reload path: function has a parameter ─────────────────────────
    //
    // If main takes a parameter (e.g. `env: AppEnv`), we generate two cfg-gated
    // code paths: one that calls the setup function directly, and one that wraps
    // it with `serve_with_hotreload` for Subsecond hot-patching.
    //
    // The setup function is either explicitly specified via `#[r2e::main(my_setup)]`
    // or defaults to `setup` by convention.
    if let Some(FnArg::Typed(param)) = sig.inputs.first() {
        let setup_fn: syn::Path = args.setup_fn.unwrap_or_else(|| {
            syn::parse_str("setup").unwrap()
        });

        let env_pat = &param.pat;
        let env_ty = &param.ty;
        let body_stmts = &body.stmts;
        let devtools = r2e_devtools_path();

        return quote! {
            // Named function in the tip crate — Subsecond can discover and patch it.
            #[cfg(feature = "dev-reload")]
            async fn __r2e_server(#env_pat: #env_ty) {
                #(#body_stmts)*
            }

            #(#attrs)*
            #test_attr
            #vis fn #fn_name() #ret {
                #tracing_init
                #builder_fn
                    #worker_threads
                    .enable_all()
                    .build()
                    .expect("failed to build tokio runtime")
                    .block_on(async {
                        #[cfg(not(feature = "dev-reload"))]
                        {
                            let #env_pat: #env_ty = #setup_fn().await;
                            #(#body_stmts)*
                        }
                        #[cfg(feature = "dev-reload")]
                        {
                            let __r2e_env: #env_ty = #setup_fn().await;
                            // On each patch, the old server future is dropped and
                            // __r2e_server is called again with the latest code.
                            // The closure must remain non-capturing (ZST) so that
                            // subsecond's HotFn dispatches through the jump table
                            // correctly — pointer-sized closures trigger the wrong
                            // call_as_ptr path.
                            #devtools::serve_with_hotreload_env(
                                __r2e_env,
                                |__r2e_arg| __r2e_server(__r2e_arg),
                            ).await;
                        }
                    })
            }
        };
    }

    // setup_fn specified but no parameter on main → error
    if args.setup_fn.is_some() {
        return syn::Error::new_spanned(
            sig,
            "#[r2e::main(setup_fn)] requires a parameter, e.g.: \
             async fn main(env: AppEnv) { ... }",
        )
        .to_compile_error();
    }

    // ── Standard path: no setup function ─────────────────────────────────
    quote! {
        #(#attrs)*
        #test_attr
        #vis fn #fn_name() #ret {
            #tracing_init
            #builder_fn
                #worker_threads
                .enable_all()
                .build()
                .expect("failed to build tokio runtime")
                .block_on(async #body)
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
