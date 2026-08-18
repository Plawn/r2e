//! `#[r2e::test_suite]` on an inherent impl block.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::punctuated::Punctuated;
use syn::{
    parse_macro_input, AngleBracketedGenericArguments, Attribute, FnArg, GenericArgument, ImplItem,
    ImplItemFn, ItemImpl, Meta, Pat, PathArguments, ReturnType, Type,
};

use crate::util::crate_path::{r2e_core_path, r2e_test_path};
use crate::util::runtime_args::{parse_bool, type_ends_with, RuntimeArgs};

struct SuiteArgs {
    tracing: bool,
    app_ty: Option<syn::Path>,
    with_expr: Option<syn::Expr>,
    jwt: bool,
    /// Shared Tokio `runtime::Builder` knobs (`flavor`, `worker_threads`, …).
    runtime: RuntimeArgs,
}

impl Default for SuiteArgs {
    fn default() -> Self {
        Self {
            tracing: true,
            app_ty: None,
            with_expr: None,
            jwt: true,
            runtime: RuntimeArgs::default(),
        }
    }
}

impl SuiteArgs {
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

            if !meta.input.peek(syn::Token![=]) {
                return Err(meta.error(format!("unexpected argument `{key}`")));
            }

            match key.as_str() {
                "tracing" => this.tracing = parse_bool(&meta)?,
                "app" => this.app_ty = Some(meta.value()?.parse()?),
                "with" => this.with_expr = Some(meta.value()?.parse()?),
                "jwt" => this.jwt = parse_bool(&meta)?,
                _ => {
                    if !this.runtime.try_parse(&key, &meta)? {
                        return Err(meta.error(format!("unknown argument `{key}`")));
                    }
                }
            }
            Ok(())
        });

        syn::parse::Parser::parse(parser, args)?;
        Ok(this)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SuiteAttr {
    Case,
    BeforeAll,
    BeforeEach,
    AfterEach,
    AfterAll,
}

struct CaseMethod {
    method: ImplItemFn,
    order: Option<u32>,
}

struct SuiteDef {
    item_impl: ItemImpl,
    self_ty: Type,
    suite_ident: syn::Ident,
    before_all: Option<ImplItemFn>,
    before_each: Vec<ImplItemFn>,
    after_each: Vec<ImplItemFn>,
    after_all: Option<ImplItemFn>,
    cases: Vec<CaseMethod>,
}

pub fn expand(args: TokenStream, input: TokenStream) -> TokenStream {
    let parsed_args = match SuiteArgs::parse(args) {
        Ok(args) => args,
        Err(err) => return err.to_compile_error().into(),
    };
    let item_impl = parse_macro_input!(input as ItemImpl);
    match expand_inner(parsed_args, item_impl) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

fn expand_inner(args: SuiteArgs, item_impl: ItemImpl) -> syn::Result<TokenStream2> {
    if item_impl.trait_.is_some() {
        return Err(syn::Error::new_spanned(
            &item_impl.impl_token,
            "#[r2e::test_suite] only supports inherent impl blocks",
        ));
    }
    if item_impl.generics.lt_token.is_some()
        || !item_impl.generics.params.is_empty()
        || item_impl.generics.where_clause.is_some()
    {
        return Err(syn::Error::new_spanned(
            &item_impl.generics,
            "#[r2e::test_suite] does not support generic impl blocks yet",
        ));
    }
    if args.app_ty.is_none() && (args.with_expr.is_some() || !args.jwt) {
        return Err(syn::Error::new_spanned(
            &item_impl.impl_token,
            "`with = ...` and `jwt = ...` require `app = <App type>`",
        ));
    }

    let mut def = parse_suite(item_impl)?;
    validate_suite(&def)?;

    let cleaned_impl = clean_impl(&mut def.item_impl);
    let generated = generate_suite(&args, &def)?;
    Ok(quote! {
        #cleaned_impl
        #generated
    })
}

fn parse_suite(item_impl: ItemImpl) -> syn::Result<SuiteDef> {
    let self_ty = (*item_impl.self_ty).clone();
    let suite_ident = suite_ident(&self_ty)?;
    let mut before_all = None;
    let mut before_each = Vec::new();
    let mut after_each = Vec::new();
    let mut after_all = None;
    let mut cases = Vec::new();

    for item in &item_impl.items {
        let ImplItem::Fn(method) = item else {
            continue;
        };
        let attrs = suite_attrs(&method.attrs);
        if attrs.is_empty() {
            continue;
        }
        if attrs.len() > 1 {
            return Err(syn::Error::new_spanned(
                &method.sig.ident,
                "a test-suite method can have only one of #[case], #[before_all], #[before_each], #[after_each], or #[after_all]",
            ));
        }
        match attrs[0] {
            SuiteAttr::Case => {
                cases.push(CaseMethod {
                    method: method.clone(),
                    order: case_order(method)?,
                });
            }
            SuiteAttr::BeforeAll => {
                if before_all.replace(method.clone()).is_some() {
                    return Err(syn::Error::new_spanned(
                        &method.sig.ident,
                        "only one #[before_all] method is allowed per test suite",
                    ));
                }
            }
            SuiteAttr::BeforeEach => before_each.push(method.clone()),
            SuiteAttr::AfterEach => after_each.push(method.clone()),
            SuiteAttr::AfterAll => {
                if after_all.replace(method.clone()).is_some() {
                    return Err(syn::Error::new_spanned(
                        &method.sig.ident,
                        "only one #[after_all] method is allowed per test suite",
                    ));
                }
            }
        }
    }

    Ok(SuiteDef {
        item_impl,
        self_ty,
        suite_ident,
        before_all,
        before_each,
        after_each,
        after_all,
        cases,
    })
}

fn validate_suite(def: &SuiteDef) -> syn::Result<()> {
    if def.cases.is_empty() {
        return Err(syn::Error::new_spanned(
            &def.item_impl.impl_token,
            "#[r2e::test_suite] requires at least one #[case] method",
        ));
    }

    let mut seen_orders = std::collections::BTreeMap::<u32, &syn::Ident>::new();
    for case in &def.cases {
        validate_receiver_only(&case.method, "#[case]")?;
        if let Some(order) = case.order {
            if let Some(first) = seen_orders.insert(order, &case.method.sig.ident) {
                return Err(syn::Error::new_spanned(
                    &case.method.sig.ident,
                    format!(
                        "duplicate #[case(order = {order})] in this test suite: both `{first}` and `{}` claim it",
                        case.method.sig.ident
                    ),
                ));
            }
        }
    }
    for method in &def.before_each {
        validate_receiver_only(method, "#[before_each]")?;
    }
    for method in &def.after_each {
        validate_receiver_only(method, "#[after_each]")?;
    }
    if let Some(method) = &def.after_all {
        validate_receiver_only(method, "#[after_all]")?;
    }
    if let Some(method) = &def.before_all {
        if method
            .sig
            .inputs
            .iter()
            .any(|arg| matches!(arg, FnArg::Receiver(_)))
        {
            return Err(syn::Error::new_spanned(
                &method.sig.ident,
                "#[before_all] must be an associated function without `self`; return `Self` to construct the suite, or return `()` and require `Default`",
            ));
        }
        for input in &method.sig.inputs {
            validate_bindable_param(input, "#[before_all]")?;
        }
    }
    Ok(())
}

fn validate_receiver_only(method: &ImplItemFn, attr: &str) -> syn::Result<()> {
    let mut inputs = method.sig.inputs.iter();
    match inputs.next() {
        Some(FnArg::Receiver(_)) => {}
        Some(other) => {
            return Err(syn::Error::new_spanned(
                other,
                format!("{attr} methods must take `&self` or `&mut self` and no other parameters"),
            ));
        }
        None => {
            return Err(syn::Error::new_spanned(
                &method.sig.ident,
                format!("{attr} methods must take `&self` or `&mut self`"),
            ));
        }
    }
    if let Some(extra) = inputs.next() {
        return Err(syn::Error::new_spanned(
            extra,
            format!("{attr} methods must take `&self` or `&mut self` and no other parameters"),
        ));
    }
    Ok(())
}

fn validate_bindable_param(input: &FnArg, attr: &str) -> syn::Result<()> {
    let FnArg::Typed(param) = input else {
        return Err(syn::Error::new_spanned(
            input,
            format!("{attr} does not accept `self`"),
        ));
    };
    if !matches!(&*param.pat, Pat::Ident(_)) {
        return Err(syn::Error::new_spanned(
            &param.pat,
            format!("{attr} parameters must use identifier patterns"),
        ));
    }
    Ok(())
}

fn generate_suite(args: &SuiteArgs, def: &SuiteDef) -> syn::Result<TokenStream2> {
    let test_crate = r2e_test_path();
    let core_crate = r2e_core_path();
    let suite_mod = format_ident!("__r2e_suite_{}", def.suite_ident);
    let self_ty = &def.self_ty;
    let total_cases = def.cases.len();
    let runtime_builder = args.runtime.builder_tokens();
    let tracing_init = args
        .tracing
        .then(|| quote! { #core_crate::init_tracing(); });
    let init_suite = init_suite_expr(args, def)?;
    let before_each: Vec<_> = def
        .before_each
        .iter()
        .map(|m| call_suite_method(m, &quote! { __r2e_suite }))
        .collect();
    let after_each: Vec<_> = def
        .after_each
        .iter()
        .map(|m| call_suite_method(m, &quote! { __r2e_suite }))
        .collect();
    let after_all = def
        .after_all
        .as_ref()
        .map(|m| call_suite_method(m, &quote! { __r2e_suite }));
    let tests = def.cases.iter().map(|case| {
        generate_case(
            def,
            case,
            &before_each,
            &after_each,
            after_all.as_ref(),
            &runtime_builder,
            &tracing_init,
        )
    });

    Ok(quote! {
        #[allow(non_snake_case)]
        mod #suite_mod {
            use super::*;

            static __R2E_SUITE: ::std::sync::OnceLock<#test_crate::suite::SuiteCell<#self_ty>> =
                ::std::sync::OnceLock::new();

            fn __r2e_suite_cell() -> &'static #test_crate::suite::SuiteCell<#self_ty> {
                __R2E_SUITE.get_or_init(|| #test_crate::suite::SuiteCell::new(#total_cases))
            }

            async fn __r2e_init_suite() -> #self_ty {
                #init_suite
            }

            #(#tests)*
        }
    })
}

fn generate_case(
    def: &SuiteDef,
    case: &CaseMethod,
    before_each: &[TokenStream2],
    after_each: &[TokenStream2],
    after_all: Option<&TokenStream2>,
    runtime_builder: &TokenStream2,
    tracing_init: &Option<TokenStream2>,
) -> TokenStream2 {
    let test_crate = r2e_test_path();
    let method = &case.method;
    let fn_name = &method.sig.ident;
    let suite_ident = &def.suite_ident;
    let generated_attrs: Vec<_> = method
        .attrs
        .iter()
        .filter(|attr| !is_suite_attr(attr))
        .cloned()
        .collect();
    let case_call = call_suite_method(method, &quote! { __r2e_suite });
    let should_panic = method
        .attrs
        .iter()
        .any(|a| a.path().is_ident("should_panic"));
    let order_submit = case.order.map(|order| {
        quote! {
            #test_crate::ordering::inventory::submit! {
                #test_crate::ordering::OrderedTestEntry {
                    group: concat!(module_path!(), "::", stringify!(#suite_ident), "::suite_order"),
                    order: #order,
                    test: concat!(module_path!(), "::", stringify!(#fn_name)),
                }
            }
        }
    });
    let order_turn = case.order.map(|order| {
        let expect_panic = should_panic.then(|| quote! { __r2e_ordered_turn.expect_panic(); });
        quote! {
            let mut __r2e_ordered_turn = #test_crate::ordering::turn(
                concat!(module_path!(), "::", stringify!(#suite_ident), "::suite_order"),
                #order,
                concat!(module_path!(), "::", stringify!(#fn_name)),
            );
            #expect_panic
        }
    });
    let maybe_mark_failed = case.order.filter(|_| !should_panic).map(|_| {
        quote! {
            if __r2e_case_panicked {
                __r2e_ordered_turn.mark_failed();
            }
        }
    });
    let after_all_call = after_all.map(|call| {
        quote! {
            if __r2e_run_after_all {
                let __r2e_suite = __r2e_state
                    .suite
                    .as_mut()
                    .expect("R2E test suite was initialized");
                let __r2e_after_all_result = ::std::panic::catch_unwind(
                    ::std::panic::AssertUnwindSafe(|| {
                        __r2e_runtime.block_on(async {
                            let __r2e_after_all_outcome = #call;
                            #test_crate::suite::SuiteOutcome::assert_passed(__r2e_after_all_outcome);
                        })
                    }),
                );
                if __r2e_resume.is_none() {
                    __r2e_resume = __r2e_after_all_result.err();
                }
            }
        }
    });

    quote! {
        #order_submit
        #(#generated_attrs)*
        #[::core::prelude::v1::test]
        fn #fn_name() {
            #tracing_init
            #order_turn
            let __r2e_runtime = #runtime_builder;
            let __r2e_cell = __r2e_suite_cell();
            let mut __r2e_state = __r2e_cell.lock();
            if __r2e_state.init_failed {
                panic!("R2E test suite initialization failed in a previous case");
            }
            if __r2e_state.suite.is_none() {
                let __r2e_init_result = ::std::panic::catch_unwind(
                    ::std::panic::AssertUnwindSafe(|| {
                        __r2e_runtime.block_on(__r2e_init_suite())
                    }),
                );
                match __r2e_init_result {
                    ::core::result::Result::Ok(__r2e_suite_value) => {
                        __r2e_state.suite = Some(__r2e_suite_value);
                    }
                    ::core::result::Result::Err(__r2e_panic) => {
                        __r2e_state.init_failed = true;
                        ::std::panic::resume_unwind(__r2e_panic);
                    }
                }
            }
            let (__r2e_case_result, __r2e_after_each_result) = {
                let __r2e_suite = __r2e_state
                    .suite
                    .as_mut()
                    .expect("R2E test suite was initialized");

                let __r2e_case_result = ::std::panic::catch_unwind(
                    ::std::panic::AssertUnwindSafe(|| {
                        __r2e_runtime.block_on(async {
                            #(
                                let __r2e_before_each_outcome = #before_each;
                                #test_crate::suite::SuiteOutcome::assert_passed(__r2e_before_each_outcome);
                            )*
                            let __r2e_case_outcome = #case_call;
                            #test_crate::suite::SuiteOutcome::assert_passed(__r2e_case_outcome);
                        })
                    }),
                );

                let __r2e_after_each_result = ::std::panic::catch_unwind(
                    ::std::panic::AssertUnwindSafe(|| {
                        __r2e_runtime.block_on(async {
                            #(
                                let __r2e_after_each_outcome = #after_each;
                                #test_crate::suite::SuiteOutcome::assert_passed(__r2e_after_each_outcome);
                            )*
                        })
                    }),
                );

                (__r2e_case_result, __r2e_after_each_result)
            };

            __r2e_state.completed_cases += 1;
            let __r2e_run_after_all =
                __r2e_state.completed_cases == __r2e_cell.total_cases()
                    && !__r2e_state.after_all_ran;
            if __r2e_run_after_all {
                __r2e_state.after_all_ran = true;
            }

            let mut __r2e_resume = __r2e_case_result.err().or_else(|| __r2e_after_each_result.err());
            #after_all_call
            let __r2e_case_panicked = __r2e_resume.is_some();
            #maybe_mark_failed
            if let Some(__r2e_panic) = __r2e_resume {
                ::std::panic::resume_unwind(__r2e_panic);
            }
        }
    }
}

fn init_suite_expr(args: &SuiteArgs, def: &SuiteDef) -> syn::Result<TokenStream2> {
    let self_ty = &def.self_ty;
    let test_crate = r2e_test_path();
    let Some(before_all) = &def.before_all else {
        return Ok(quote! { <#self_ty as ::core::default::Default>::default() });
    };

    let (bindings, args_exprs) = before_all_bindings(args, before_all)?;
    let call = call_associated_method(before_all, self_ty, &args_exprs);
    let call = if before_all.sig.asyncness.is_some() {
        quote! { #call.await }
    } else {
        call
    };

    let expr = match before_all_kind(before_all, self_ty) {
        BeforeAllKind::ConstructsSelf => quote! {
            #(#bindings)*
            #call
        },
        BeforeAllKind::ConstructsSelfResult => quote! {
            #(#bindings)*
            match #call {
                ::core::result::Result::Ok(__r2e_suite) => __r2e_suite,
                ::core::result::Result::Err(__r2e_err) => {
                    panic!("R2E test suite #[before_all] returned Err: {__r2e_err:?}");
                }
            }
        },
        BeforeAllKind::DefaultAfterHook => quote! {
            #(#bindings)*
            let __r2e_before_all_outcome = #call;
            #test_crate::suite::SuiteOutcome::assert_passed(__r2e_before_all_outcome);
            <#self_ty as ::core::default::Default>::default()
        },
    };
    Ok(expr)
}

fn before_all_bindings(
    args: &SuiteArgs,
    method: &ImplItemFn,
) -> syn::Result<(Vec<TokenStream2>, Vec<TokenStream2>)> {
    let test_crate = r2e_test_path();
    let mut bindings = Vec::new();
    let mut args_exprs = Vec::new();
    let mut has_test_app_param = false;
    let needs_app = !method.sig.inputs.is_empty();
    if needs_app && args.app_ty.is_none() {
        return Err(syn::Error::new_spanned(
            &method.sig.ident,
            "#[before_all] parameters require #[r2e::test_suite(app = <App type>)]",
        ));
    }

    if let Some(app_ty) = &args.app_ty {
        if needs_app {
            let configure: TokenStream2 = match &args.with_expr {
                Some(expr) => quote! { #expr },
                None => quote! { |__r2e_b| __r2e_b },
            };
            let boot_call = if args.jwt {
                quote! { #test_crate::TestApp::boot_with::<#app_ty>(#configure).await }
            } else {
                quote! { #test_crate::TestApp::boot_plain::<#app_ty>(#configure).await }
            };
            bindings.push(quote! { let __r2e_test_app = #boot_call; });
        }
    }

    for input in &method.sig.inputs {
        let FnArg::Typed(param) = input else {
            unreachable!("validated earlier")
        };
        let ident = match &*param.pat {
            Pat::Ident(pat) => &pat.ident,
            _ => unreachable!("validated earlier"),
        };
        let pat = &param.pat;
        let ty = &param.ty;
        let is_inject = param.attrs.iter().any(|a| a.path().is_ident("inject"));
        if is_inject {
            bindings.push(quote! { let #pat: #ty = __r2e_test_app.bean::<#ty>(); });
            args_exprs.push(quote! { #ident });
        } else if type_ends_with(ty, "TestApp") {
            if has_test_app_param {
                return Err(syn::Error::new_spanned(
                    param,
                    "only one `TestApp` parameter is allowed in #[before_all]",
                ));
            }
            has_test_app_param = true;
            args_exprs.push(quote! { __r2e_test_app });
        } else if type_ends_with(ty, "TestJwt") {
            bindings.push(quote! { let #pat: #ty = __r2e_test_app.test_jwt().clone(); });
            args_exprs.push(quote! { #ident });
        } else {
            return Err(syn::Error::new_spanned(
                param,
                "#[before_all] parameters must be `TestApp`, `TestJwt`, or a bean marked `#[inject]`",
            ));
        }
    }

    Ok((bindings, args_exprs))
}

enum BeforeAllKind {
    ConstructsSelf,
    ConstructsSelfResult,
    DefaultAfterHook,
}

fn before_all_kind(method: &ImplItemFn, self_ty: &Type) -> BeforeAllKind {
    match &method.sig.output {
        ReturnType::Default => BeforeAllKind::DefaultAfterHook,
        ReturnType::Type(_, ty) if type_is_self(ty, self_ty) => BeforeAllKind::ConstructsSelf,
        ReturnType::Type(_, ty) if result_inner_is_self(ty, self_ty) => {
            BeforeAllKind::ConstructsSelfResult
        }
        ReturnType::Type(_, _) => BeforeAllKind::DefaultAfterHook,
    }
}

fn call_suite_method(method: &ImplItemFn, receiver: &TokenStream2) -> TokenStream2 {
    let ident = &method.sig.ident;
    let call = quote! { #receiver.#ident() };
    if method.sig.asyncness.is_some() {
        quote! { #call.await }
    } else {
        call
    }
}

fn call_associated_method(
    method: &ImplItemFn,
    self_ty: &Type,
    args_exprs: &[TokenStream2],
) -> TokenStream2 {
    let ident = &method.sig.ident;
    quote! { <#self_ty>::#ident(#(#args_exprs),*) }
}

fn clean_impl(item_impl: &mut ItemImpl) -> TokenStream2 {
    for item in &mut item_impl.items {
        let ImplItem::Fn(method) = item else {
            continue;
        };
        method.attrs.retain(|attr| {
            !is_suite_attr(attr)
                && !attr.path().is_ident("ignore")
                && !attr.path().is_ident("should_panic")
        });
        for input in &mut method.sig.inputs {
            let FnArg::Typed(param) = input else {
                continue;
            };
            param.attrs.retain(|attr| !attr.path().is_ident("inject"));
        }
    }
    quote! { #item_impl }
}

fn suite_attrs(attrs: &[Attribute]) -> Vec<SuiteAttr> {
    attrs.iter().filter_map(suite_attr).collect()
}

fn suite_attr(attr: &Attribute) -> Option<SuiteAttr> {
    let ident = attr.path().get_ident()?.to_string();
    match ident.as_str() {
        "case" => Some(SuiteAttr::Case),
        "before_all" | "beforeAll" => Some(SuiteAttr::BeforeAll),
        "before_each" | "beforeEach" => Some(SuiteAttr::BeforeEach),
        "after_each" | "afterEach" => Some(SuiteAttr::AfterEach),
        "after_all" | "afterAll" => Some(SuiteAttr::AfterAll),
        _ => None,
    }
}

fn is_suite_attr(attr: &Attribute) -> bool {
    suite_attr(attr).is_some()
}

fn case_order(method: &ImplItemFn) -> syn::Result<Option<u32>> {
    let Some(attr) = method
        .attrs
        .iter()
        .find(|attr| matches!(suite_attr(attr), Some(SuiteAttr::Case)))
    else {
        return Ok(None);
    };
    if matches!(attr.meta, Meta::Path(_)) {
        return Ok(None);
    }
    let mut order = None;
    attr.parse_nested_meta(|meta| {
        let key = meta
            .path
            .get_ident()
            .map(|i| i.to_string())
            .unwrap_or_default();
        match key.as_str() {
            "order" => {
                let lit: syn::LitInt = meta.value()?.parse()?;
                order = Some(lit.base10_parse()?);
                Ok(())
            }
            _ => Err(meta.error(format!("unknown #[case] argument `{key}`"))),
        }
    })?;
    Ok(order)
}

fn suite_ident(ty: &Type) -> syn::Result<syn::Ident> {
    if let Type::Path(path) = ty {
        if let Some(segment) = path.path.segments.last() {
            return Ok(segment.ident.clone());
        }
    }
    Err(syn::Error::new_spanned(
        ty,
        "#[r2e::test_suite] requires an impl for a concrete path type",
    ))
}

fn type_is_self(ty: &Type, self_ty: &Type) -> bool {
    if type_ends_with(ty, "Self") {
        return true;
    }
    quote! { #ty }.to_string() == quote! { #self_ty }.to_string()
}

fn result_inner_is_self(ty: &Type, self_ty: &Type) -> bool {
    let Type::Path(path) = ty else {
        return false;
    };
    let Some(segment) = path.path.segments.last() else {
        return false;
    };
    if segment.ident != "Result" {
        return false;
    }
    let PathArguments::AngleBracketed(AngleBracketedGenericArguments { args, .. }) =
        &segment.arguments
    else {
        return false;
    };
    let Some(GenericArgument::Type(inner)) = first_generic_arg(args) else {
        return false;
    };
    type_is_self(inner, self_ty)
}

fn first_generic_arg(
    args: &Punctuated<GenericArgument, syn::Token![,]>,
) -> Option<&GenericArgument> {
    args.iter().next()
}
