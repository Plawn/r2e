//! `#[r2e::main]` and `#[r2e::test]` attribute macros.
//!
//! These wrap the user's `async fn main()` / `async fn test_*()` in a Tokio
//! runtime and optionally call `init_tracing()`.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{parse_macro_input, Expr, ExprLit, ItemFn, Lit, Meta};

use crate::crate_path::r2e_core_path;

// ── Argument parsing ─────────────────────────────────────────────────────

/// `None` = user did not set `flavor`, `Some(true)` = current_thread, `Some(false)` = multi_thread
struct MainArgs {
    tracing: bool,
    worker_threads: Option<usize>,
    flavor: Option<bool>,
}

impl Default for MainArgs {
    fn default() -> Self {
        Self {
            tracing: true,
            worker_threads: None,
            flavor: None,
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
                other => {
                    return Err(syn::Error::new_spanned(
                        other,
                        "expected `key = value` arguments",
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
