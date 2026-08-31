//! Automatic `+ use<...>` on handler return types.
//!
//! # Why
//!
//! A route/SSE/WS method's return value is **moved into the HTTP response**:
//! the generated invocation calls `__ctrl.handler(..).await` through a
//! borrowed receiver and then hands the value back out of a function whose own
//! return type outlives that borrow. Under edition 2024 a return-position
//! `impl Trait` captures every lifetime in scope — including the one behind
//! `&self` — so the perfectly ordinary
//!
//! ```ignore
//! #[sse("/events")]
//! async fn events(&self) -> impl Stream<Item = Result<SseEvent, Infallible>> { … }
//! ```
//!
//! fails to compile with "borrowed data escapes outside of method" / "`c` does
//! not live long enough", and the only fix is for the user to write a
//! `+ use<>` clause R2E could have written for them. Nothing about that clause
//! is a design choice on the app's part — it is boilerplate the framework's
//! own calling convention forces.
//!
//! So `#[routes]` writes it: every return-position `impl Trait` in a **handler**
//! signature that does not already carry a precise-capture clause gets
//! `+ use<…>` naming the method's own type and const parameters, and no
//! lifetimes.
//!
//! # Why only handlers, and why this is not lossy
//!
//! It is applied to route / `#[sse]` / `#[ws]` methods **only** — never to
//! `#[request_helper]`s, off-request helpers, consumers or scheduled methods,
//! where returning a value that borrows `&self` is a legitimate thing to do.
//! For a handler it is not: the value outlives the borrow by construction, so
//! there is no capture to lose. That is what makes appending the clause safe
//! rather than merely convenient.
//!
//! # When it declines
//!
//! `use<...>` is a hard error in three shapes, so the rewrite skips a signature
//! that is in any of them and leaves the author in charge:
//!
//! - **an explicit clause is already there** — `use<'a>` on the handler wins,
//!   which is also the documented escape hatch;
//! - **the `impl Trait` bounds mention a lifetime or a reference**
//!   (`impl Iterator<Item = &str>`, `impl Future<Output = ()> + 'a`): the
//!   lifetime is captured *because* it is named in the bounds, and omitting it
//!   from `use<...>` is rejected outright;
//! - **the signature has an argument-position `impl Trait`**
//!   (`fn h(&self, x: impl Into<String>)`): that desugars to an unnameable type
//!   parameter, and `use<...>` must list every type parameter in scope.
//!
//! In each case the handler compiles exactly as it did before, and the compiler
//! prints the same suggestion it prints for hand-written code.

use proc_macro2::TokenTree;
use syn::visit_mut::{self, VisitMut};

/// Rewrite a **handler** signature to add `+ use<...>` where it is safe to.
///
/// No-op for every signature described under "When it declines" above.
pub fn add_handler_precise_captures(sig: &mut syn::Signature) {
    let syn::ReturnType::Type(_, ret) = &mut sig.output else {
        return;
    };
    // Argument-position `impl Trait` introduces a type parameter with no name
    // to put in the list, which makes any `use<...>` on this signature an
    // error. Leave the whole signature alone.
    if sig.inputs.iter().any(|arg| match arg {
        syn::FnArg::Typed(pt) => contains_impl_trait(&pt.ty),
        syn::FnArg::Receiver(_) => false,
    }) {
        return;
    }
    // Every type/const parameter in scope must be listed; lifetimes are the
    // ones we are deliberately dropping. The impl block `#[routes]` splits is
    // always `impl <ControllerIdent>` (no generics of its own), so the
    // method's own parameters are the whole scope.
    let params: Vec<syn::Ident> = sig
        .generics
        .params
        .iter()
        .filter_map(|p| match p {
            syn::GenericParam::Type(t) => Some(t.ident.clone()),
            syn::GenericParam::Const(c) => Some(c.ident.clone()),
            syn::GenericParam::Lifetime(_) => None,
        })
        .collect();

    AddCaptures { params }.visit_type_mut(ret);
}

/// The handler's return type with the same rewrite applied, for the sites that
/// re-emit it on a **generated** signature.
///
/// The generated invocation function (`__r2e_invoke_<Ctrl>_<method>`) copies the
/// user's return type verbatim onto a `fn(&__R2eRequest_<Ctrl>) -> …`, so it
/// needs the clause for exactly the same reason the façade method does: the
/// tokens come from the user's crate and are therefore read under *its* edition,
/// while everything `quote!` produces here is read under this crate's. Rewriting
/// only the façade method leaves the invocation function failing on its own.
pub fn handler_return_type(sig: &syn::Signature) -> syn::ReturnType {
    let mut sig = sig.clone();
    add_handler_precise_captures(&mut sig);
    sig.output
}

struct AddCaptures {
    params: Vec<syn::Ident>,
}

impl VisitMut for AddCaptures {
    fn visit_type_impl_trait_mut(&mut self, node: &mut syn::TypeImplTrait) {
        // Nested first, so `Sse<impl Stream<…>>` gets the clause on the inner
        // `impl Trait` — the one the response type is generic over.
        visit_mut::visit_type_impl_trait_mut(self, node);

        if node
            .bounds
            .iter()
            .any(|b| matches!(b, syn::TypeParamBound::PreciseCapture(_)))
        {
            return;
        }
        if bounds_mention_lifetime(&node.bounds) {
            return;
        }
        let params = &self.params;
        let clause: syn::TypeParamBound = syn::parse_quote!(use<#(#params),*>);
        node.bounds.push(clause);
    }
}

/// Whether a type contains a `impl Trait` anywhere inside it.
fn contains_impl_trait(ty: &syn::Type) -> bool {
    struct Probe(bool);
    impl<'ast> syn::visit::Visit<'ast> for Probe {
        fn visit_type_impl_trait(&mut self, node: &'ast syn::TypeImplTrait) {
            self.0 = true;
            syn::visit::visit_type_impl_trait(self, node);
        }
    }
    let mut probe = Probe(false);
    syn::visit::Visit::visit_type(&mut probe, ty);
    probe.0
}

/// Whether the bounds name a lifetime or a reference at any depth.
///
/// Both make the `impl Trait` capture a lifetime that a `use<...>` list without
/// it would reject, so they are the signal to decline. Scanning tokens rather
/// than the typed AST is deliberate: `'a`, `&T`, `&'a T`, `Item = &str`, a
/// lifetime buried in an associated-type binding and a `+ '_` bound all reduce
/// to the same two token shapes wherever they appear.
fn bounds_mention_lifetime(
    bounds: &syn::punctuated::Punctuated<syn::TypeParamBound, syn::Token![+]>,
) -> bool {
    fn scan(tokens: proc_macro2::TokenStream) -> bool {
        tokens.into_iter().any(|tt| match tt {
            TokenTree::Punct(p) => p.as_char() == '\'' || p.as_char() == '&',
            TokenTree::Group(g) => scan(g.stream()),
            _ => false,
        })
    }
    scan(quote::ToTokens::to_token_stream(bounds))
}
