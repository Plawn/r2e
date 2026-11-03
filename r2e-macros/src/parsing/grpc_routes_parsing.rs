//! Parsing for the `#[grpc_routes(TraitPath)]` attribute macro.


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

        // `#[request_helper]` is classified first: a helper may be `async`
        // and take `&self`, which would otherwise read as an rpc.
        if method
            .attrs
            .iter()
            .any(|a| a.path().is_ident("request_helper"))
        {
            validate_member_attrs(&method.attrs, &rules)?;
            let mut method = substitute_self_assoc(method, &assoc_types);
            reject_self_in_route_signature(&method.sig, "#[request_helper]")?;
            method.attrs.retain(|a| !a.path().is_ident("request_helper"));
            facade_helpers.push(method);
            continue;
        }

        // Every other async method with a receiver is a gRPC method
        // (matching the tonic trait). Non-async or methods without &self are helpers.
        let is_receiver = method
            .sig
            .inputs
            .first()
            .is_some_and(|arg| matches!(arg, syn::FnArg::Receiver(_)));

        if method.sig.asyncness.is_some() && is_receiver {
            let method = substitute_self_assoc(method, &assoc_types);
            reject_self_in_route_signature(&method.sig, "#[grpc_routes]")?;
            methods.push(GrpcMethod {
                member: parse_member(method, &rules)?,
            });
        } else {
            // Pass-through helpers still can't carry gRPC-disallowed
            // markers: a sync `#[pre_destroy] fn close(&self)` would
            // otherwise be re-emitted verbatim and silently never run.
            validate_member_attrs(&method.attrs, &rules)?;
            other_items.push(syn::ImplItem::Fn(method));
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
///
/// Structural, not token-level:
/// - a type `Self::X` becomes the declared type;
/// - a longer path `Self::X::rest` outside a type becomes the declared path
///   in expression form, generics as turbofish
///   (`Self::WatchStream::new(rx)` → `ReceiverStream::<..>::new(rx)`,
///   `Self::X::Done { .. }` → `Replies::Done { .. }`), which is valid in an
///   expression, a struct literal and a pattern alike — so an expression
///   that a macro expands into a pattern (`matches!`) stays valid too. When
///   the declared type is not a plain path, an expression gets the
///   qualified `<Declared>::rest` instead; a struct literal or pattern
///   (where qualified paths are not stable, `more_qualified_paths`) is left
///   as written and fails to resolve on the façade (E0223). In a type,
///   `Self::X::rest` becomes `<Declared>::rest`. The generic arguments kept
///   in `rest` (`::wrap::<Self::X>`) are visited in turn;
/// - a bare `Self::X` outside a type becomes the expression-form declared
///   path (when it is a plain path);
/// - macro arguments are re-parsed as a comma-separated expression list
///   (`dbg!(..)`, `vec![..]`, `assert_eq!(..)`, `matches!(x, Self::X::A(_))`)
///   and rewritten structurally; a macro body that does not parse that way
///   (`matches!(x, P if guard)`, custom DSLs) falls back to a token-level
///   rewrite of the `Self :: X` prefix — the expression-form declared path
///   (or `<Declared>`) before a `::`, the declared type otherwise — which can
///   mis-handle a generic declared type used as a bare value in such a body.
///
/// Items nested in the body (`impl`, `trait`, `fn`, `struct`, …) are not
/// entered: each introduces its own `Self` (or cannot name the outer one),
/// so the outer impl's associated types do not apply there. Closures and
/// (async) blocks are entered.
fn substitute_self_assoc(
    mut method: syn::ImplItemFn,
    assoc_types: &[syn::ImplItemType],
) -> syn::ImplItemFn {
    use syn::visit_mut::{self, VisitMut};

    if assoc_types.is_empty() {
        return method;
    }

    struct Subst<'a> {
        assoc: &'a [syn::ImplItemType],
    }

    /// Where a `Self::X…` path sits.
    #[derive(Clone, Copy, PartialEq)]
    enum Pos {
        /// A type: `<Declared>::rest`.
        Type,
        /// An expression path: the expression-form path, else `<Declared>::rest`.
        Expr,
        /// A struct literal or struct / tuple-struct pattern: the
        /// expression-form path only (qualified paths are unstable there).
        Literal,
    }

    /// What a `Self::X…` path was rewritten to.
    enum Rewritten {
        /// Nothing to rewrite.
        No,
        /// `Self::X::rest` rewritten in place.
        Done,
        /// `Self::X` alone: the declared type, for the caller to place.
        Bare(Box<syn::Type>),
    }

    impl Subst<'_> {
        /// `Self::X…` with `X` an associated type of the impl: the declared
        /// type.
        fn declared(&self, qself: &Option<syn::QSelf>, path: &syn::Path) -> Option<syn::Type> {
            if qself.is_some() || path.leading_colon.is_some() || path.segments.len() < 2 {
                return None;
            }
            let mut segs = path.segments.iter();
            let first = segs.next()?;
            if first.ident != "Self" || !first.arguments.is_none() {
                return None;
            }
            let second = segs.next()?;
            // `Self::X::<>` is `Self::X`; non-empty arguments (a GAT) are
            // not rewritten.
            match &second.arguments {
                syn::PathArguments::None => {}
                syn::PathArguments::AngleBracketed(args) if args.args.is_empty() => {}
                _ => return None,
            }
            let t = self.assoc.iter().find(|t| t.ident == second.ident)?;
            Some(t.ty.clone())
        }

        /// A plain-path declared type as an expression path (generics as
        /// turbofish): `Foo<T>` → `Foo::<T>`.
        fn expr_path(declared: &syn::Type) -> Option<syn::Path> {
            let syn::Type::Path(tp) = declared else {
                return None;
            };
            if tp.qself.is_some() {
                return None;
            }
            let mut p = tp.path.clone();
            for seg in p.segments.iter_mut() {
                if let syn::PathArguments::AngleBracketed(args) = &mut seg.arguments {
                    args.colon2_token = Some(Default::default());
                }
            }
            Some(p)
        }

        /// Rewrite `Self::X::rest` in place for its position, then visit
        /// what `rest` keeps (its generic arguments).
        fn rewrite(
            &mut self,
            qself: &mut Option<syn::QSelf>,
            path: &mut syn::Path,
            pos: Pos,
        ) -> Rewritten {
            let Some(declared) = self.declared(qself, path) else {
                return Rewritten::No;
            };
            if path.segments.len() == 2 {
                return Rewritten::Bare(Box::new(declared));
            }
            let rest: syn::punctuated::Punctuated<syn::PathSegment, syn::Token![::]> =
                path.segments.iter().skip(2).cloned().collect();
            let plain = if pos == Pos::Type {
                None
            } else {
                Self::expr_path(&declared)
            };
            let skip = match plain {
                Some(mut p) => {
                    let n = p.segments.len();
                    p.segments.extend(rest);
                    *path = p;
                    n
                }
                None if pos == Pos::Literal => return Rewritten::No,
                None => {
                    *qself = Some(syn::QSelf {
                        lt_token: Default::default(),
                        ty: Box::new(declared),
                        position: 0,
                        as_token: None,
                        gt_token: Default::default(),
                    });
                    path.leading_colon = Some(Default::default());
                    path.segments = rest;
                    0
                }
            };
            for seg in path.segments.iter_mut().skip(skip) {
                self.visit_path_segment_mut(seg);
            }
            Rewritten::Done
        }

        /// A path outside a type. `Self::X` alone becomes the declared path
        /// in expression form (left untouched when the declared type is not
        /// a plain path). Returns whether the path was a `Self::X…` one.
        fn path_position(
            &mut self,
            qself: &mut Option<syn::QSelf>,
            path: &mut syn::Path,
            pos: Pos,
        ) -> bool {
            match self.rewrite(qself, path, pos) {
                Rewritten::No => false,
                Rewritten::Done => true,
                Rewritten::Bare(declared) => {
                    if let Some(p) = Self::expr_path(&declared) {
                        *path = p;
                    }
                    true
                }
            }
        }

        /// Token-level fallback for a macro body that is not an expression
        /// list.
        fn tokens(&self, tokens: proc_macro2::TokenStream) -> proc_macro2::TokenStream {
            use proc_macro2::{Group, TokenTree};
            use quote::ToTokens;

            let trees: Vec<TokenTree> = tokens.into_iter().collect();
            let mut out = proc_macro2::TokenStream::new();
            let is_colon2 = |i: usize| {
                matches!((trees.get(i), trees.get(i + 1)),
                    (Some(TokenTree::Punct(a)), Some(TokenTree::Punct(b)))
                        if a.as_char() == ':' && b.as_char() == ':'
                            && a.spacing() == proc_macro2::Spacing::Joint)
            };
            let is_punct = |i: usize, c: char| {
                matches!(trees.get(i), Some(TokenTree::Punct(p)) if p.as_char() == c)
            };
            let mut i = 0;
            while i < trees.len() {
                if let TokenTree::Ident(id) = &trees[i]
                    && id == "Self"
                    && is_colon2(i + 1)
                    && let Some(TokenTree::Ident(assoc)) = trees.get(i + 3)
                    && let Some(t) = self.assoc.iter().find(|t| &t.ident == assoc)
                {
                    let ty = &t.ty;
                    i += 4;
                    // `Self::X::<>` is `Self::X`: drop the empty turbofish.
                    if is_colon2(i) && is_punct(i + 2, '<') && is_punct(i + 3, '>') {
                        i += 4;
                    }
                    if is_colon2(i) {
                        match Self::expr_path(ty) {
                            Some(p) => p.to_tokens(&mut out),
                            None => out.extend(quote::quote!(<#ty>)),
                        }
                    } else {
                        ty.to_tokens(&mut out);
                    }
                    continue;
                }
                match &trees[i] {
                    TokenTree::Group(g) => {
                        let mut ng = Group::new(g.delimiter(), self.tokens(g.stream()));
                        ng.set_span(g.span());
                        out.extend([TokenTree::Group(ng)]);
                    }
                    tt => out.extend([tt.clone()]),
                }
                i += 1;
            }
            out
        }
    }

    impl VisitMut for Subst<'_> {
        fn visit_type_mut(&mut self, ty: &mut syn::Type) {
            if let syn::Type::Path(tp) = ty {
                match self.rewrite(&mut tp.qself, &mut tp.path, Pos::Type) {
                    Rewritten::No => {}
                    Rewritten::Done => return,
                    Rewritten::Bare(declared) => {
                        *ty = *declared;
                        return;
                    }
                }
            }
            visit_mut::visit_type_mut(self, ty);
        }

        /// Expression paths and unit / path patterns (`Pat::Path` is an
        /// `ExprPath`).
        fn visit_expr_path_mut(&mut self, e: &mut syn::ExprPath) {
            if !self.path_position(&mut e.qself, &mut e.path, Pos::Expr) {
                visit_mut::visit_expr_path_mut(self, e);
            }
        }

        /// Not a `Self::X…` path: the default visitor, so the path's own
        /// qself and generic arguments (`Holder::<Self::X> { .. }`) are
        /// reached too.
        fn visit_expr_struct_mut(&mut self, e: &mut syn::ExprStruct) {
            if !self.path_position(&mut e.qself, &mut e.path, Pos::Literal) {
                return visit_mut::visit_expr_struct_mut(self, e);
            }
            for field in e.fields.iter_mut() {
                self.visit_field_value_mut(field);
            }
            if let Some(rest) = &mut e.rest {
                self.visit_expr_mut(rest);
            }
        }

        fn visit_pat_struct_mut(&mut self, p: &mut syn::PatStruct) {
            if !self.path_position(&mut p.qself, &mut p.path, Pos::Literal) {
                return visit_mut::visit_pat_struct_mut(self, p);
            }
            for field in p.fields.iter_mut() {
                self.visit_field_pat_mut(field);
            }
        }

        fn visit_pat_tuple_struct_mut(&mut self, p: &mut syn::PatTupleStruct) {
            if !self.path_position(&mut p.qself, &mut p.path, Pos::Literal) {
                return visit_mut::visit_pat_tuple_struct_mut(self, p);
            }
            for elem in p.elems.iter_mut() {
                self.visit_pat_mut(elem);
            }
        }

        fn visit_macro_mut(&mut self, mac: &mut syn::Macro) {
            use quote::ToTokens;
            use syn::parse::Parser;

            let parser = syn::punctuated::Punctuated::<syn::Expr, syn::Token![,]>::parse_terminated;
            mac.tokens = match parser.parse2(mac.tokens.clone()) {
                Ok(mut exprs) => {
                    for expr in exprs.iter_mut() {
                        self.visit_expr_mut(expr);
                    }
                    exprs.into_token_stream()
                }
                Err(_) => self.tokens(std::mem::take(&mut mac.tokens)),
            };
        }

        /// A nested item has its own `Self` (impl, trait) or cannot name the
        /// outer one (fn, struct, const, …): not entered.
        fn visit_item_mut(&mut self, _item: &mut syn::Item) {}
    }

    Subst { assoc: assoc_types }.visit_impl_item_fn_mut(&mut method);
    method
}
