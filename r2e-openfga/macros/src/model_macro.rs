//! Expansion of `model!(pub mod authz = "fga/model.fga")`.
//!
//! The `.fga` file is read relative to `CARGO_MANIFEST_DIR`, parsed and
//! semantically validated with `r2e-openfga-model`, then lowered to a typed
//! module: an `FgaType` marker + `id()` constructor per type, a lowercase
//! `FgaRel` const per relation (same convention as the `path::` params), and
//! `DirectlyAssignable` impls encoding `directly_related_user_types`.
//! `include_str!` of the source file is emitted so edits retrigger the macro.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use proc_macro2::TokenStream;
use proc_macro_crate::{crate_name, FoundCrate};
use quote::{format_ident, quote};
use r2e_openfga_model::{AuthorizationModel, RelationReference, TypeDefinition};
use syn::parse::{Parse, ParseStream};
use syn::{Ident, LitStr, Token, Visibility};

/// `[pub] mod <name> = "<path>"` or `[pub] mod <name> = inline "<dsl>"`.
struct ModelInput {
    vis: Visibility,
    mod_name: Ident,
    /// `false`: the literal is a path relative to `CARGO_MANIFEST_DIR`;
    /// `true`: the literal is the DSL itself (tests, tiny models).
    inline: bool,
    literal: LitStr,
}

impl Parse for ModelInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let vis: Visibility = input.parse()?;
        input.parse::<Token![mod]>()?;
        let mod_name: Ident = input.parse()?;
        input.parse::<Token![=]>()?;
        let inline = if input.peek(Ident) {
            let marker: Ident = input.parse()?;
            if marker != "inline" {
                return Err(syn::Error::new(
                    marker.span(),
                    "model!: expected a string literal or `inline \"<dsl>\"`",
                ));
            }
            true
        } else {
            false
        };
        let literal: LitStr = input.parse()?;
        Ok(ModelInput {
            vis,
            mod_name,
            inline,
            literal,
        })
    }
}

/// Path to the `r2e_openfga` runtime types from the calling crate.
fn r2e_openfga_path() -> TokenStream {
    static CACHE: OnceLock<String> = OnceLock::new();
    let rendered = CACHE.get_or_init(|| {
        for (candidate, suffix) in [("r2e", "r2e_openfga"), ("r2e-openfga", "")] {
            if let Ok(found) = crate_name(candidate) {
                return match found {
                    FoundCrate::Itself if suffix.is_empty() => "crate".to_string(),
                    FoundCrate::Itself => format!("crate::{}", suffix),
                    FoundCrate::Name(name) if suffix.is_empty() => format!("::{}", name),
                    FoundCrate::Name(name) => format!("::{}::{}", name, suffix),
                };
            }
        }
        "::r2e_openfga".to_string()
    });
    rendered.parse().expect("crate path must be valid Rust")
}

pub fn expand(input: TokenStream) -> syn::Result<TokenStream> {
    let ModelInput {
        vis,
        mod_name,
        inline,
        literal,
    } = syn::parse2(input)?;
    let span = literal.span();

    // (source label for errors, DSL text, include_str! path for rebuild
    // tracking — none in the inline form, the DSL is already in-source)
    let (source, dsl, include_path) = if inline {
        ("<inline>".to_string(), literal.value(), None)
    } else {
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
            .map_err(|_| syn::Error::new(span, "model!: CARGO_MANIFEST_DIR is not set"))?;
        let abs_path = std::path::Path::new(&manifest_dir).join(literal.value());
        let dsl = std::fs::read_to_string(&abs_path).map_err(|e| {
            syn::Error::new(
                span,
                format!("model!: cannot read `{}`: {}", abs_path.display(), e),
            )
        })?;
        let include_path = LitStr::new(&abs_path.to_string_lossy(), span);
        (literal.value(), dsl, Some(include_path))
    };

    let model = r2e_openfga_model::parse(&dsl)
        .map_err(|e| syn::Error::new(span, format!("model!: {}:{}", source, e)))?;
    r2e_openfga_model::validate(&model).map_err(|errors| {
        let rendered: Vec<String> = errors.iter().map(|e| format!("  - {}", e)).collect();
        syn::Error::new(
            span,
            format!(
                "model!: {} is not a valid model:\n{}",
                source,
                rendered.join("\n")
            ),
        )
    })?;

    let krate = r2e_openfga_path();
    let json = serde_json::to_string(&model.to_json()).expect("model serializes");

    let names = ModuleNames::build(&model, span)?;
    let type_modules = model
        .type_definitions
        .iter()
        .map(|t| expand_type(t, &names, &krate, span))
        .collect::<syn::Result<Vec<_>>>()?;

    let dsl_const = match include_path {
        // `include_str!` (not the read text) so edits to the `.fga` retrigger
        // this macro's expansion.
        Some(path) => quote! { pub const DSL: &str = include_str!(#path); },
        None => quote! { pub const DSL: &str = #dsl; },
    };

    Ok(quote! {
        #vis mod #mod_name {
            //! Typed authorization API generated by `r2e_openfga::model!`.

            /// The source `.fga` model.
            #dsl_const
            /// The model as schema 1.1 JSON — the `WriteAuthorizationModel`
            /// payload, for boot-time apply/verify.
            pub const MODEL: &str = #json;

            #(#type_modules)*
        }
    })
}

/// Rust-ident forms of every type and relation name, collision-checked.
struct ModuleNames {
    /// type name → module ident
    types: BTreeMap<String, Ident>,
    /// (type name, relation name) → (const ident, marker ident)
    relations: BTreeMap<(String, String), (Ident, Ident)>,
}

impl ModuleNames {
    fn build(model: &AuthorizationModel, span: proc_macro2::Span) -> syn::Result<Self> {
        let mut types = BTreeMap::new();
        let mut type_idents: BTreeMap<String, String> = BTreeMap::new();
        for t in &model.type_definitions {
            let ident_str = sanitize(&t.type_name);
            if let Some(other) = type_idents.insert(ident_str.clone(), t.type_name.clone()) {
                return Err(syn::Error::new(
                    span,
                    format!(
                        "model!: types `{}` and `{}` both map to module `{}`",
                        other, t.type_name, ident_str
                    ),
                ));
            }
            types.insert(t.type_name.clone(), make_ident(&ident_str, span, "type")?);
        }

        let mut relations = BTreeMap::new();
        for t in &model.type_definitions {
            let mut seen: BTreeMap<String, String> = BTreeMap::new();
            for rel in t.relations.keys() {
                let const_str = sanitize(rel);
                let marker_str = pascal_case(rel);
                for reserved in ["Ty", "id", "try_id", "wildcard"] {
                    if const_str == *reserved || marker_str == *reserved {
                        return Err(syn::Error::new(
                            span,
                            format!(
                                "model!: relation `{}` on type `{}` collides with the generated `{}` item",
                                rel, t.type_name, reserved
                            ),
                        ));
                    }
                }
                // An uppercase-first relation (`define Viewer:`) would make
                // the const and its own marker struct share one name.
                if const_str == marker_str {
                    return Err(syn::Error::new(
                        span,
                        format!(
                            "model!: relation `{}` on type `{}` collides with its generated marker \
                             struct `{}` — use a lowercase relation name",
                            rel, t.type_name, marker_str
                        ),
                    ));
                }
                if let Some(other) = seen.insert(marker_str.clone(), rel.clone()) {
                    return Err(syn::Error::new(
                        span,
                        format!(
                            "model!: relations `{}` and `{}` on type `{}` both map to marker `{}`",
                            other, rel, t.type_name, marker_str
                        ),
                    ));
                }
                relations.insert(
                    (t.type_name.clone(), rel.clone()),
                    (
                        make_ident(&const_str, span, "relation")?,
                        make_ident(&marker_str, span, "relation")?,
                    ),
                );
            }
        }

        Ok(ModuleNames { types, relations })
    }
}

fn expand_type(
    type_def: &TypeDefinition,
    names: &ModuleNames,
    krate: &TokenStream,
    span: proc_macro2::Span,
) -> syn::Result<TokenStream> {
    let type_name = &type_def.type_name;
    let mod_ident = &names.types[type_name];
    let ty_doc = format!("Marker for the `{}` object type.", type_name);
    let id_doc = format!(
        "`{}:<id>` — panics if `id` contains `:`; see `try_id`.",
        type_name
    );
    let wildcard_doc = format!("The public wildcard subject `{}:*`.", type_name);

    let relations = type_def
        .relations
        .keys()
        .map(|rel| {
            let (const_ident, marker_ident) = &names.relations[&(type_name.clone(), rel.clone())];
            let marker_doc = format!("Marker for the `{}` relation on `{}`.", rel, type_name);
            let const_doc = format!("The `{}` relation on `{}`.", rel, type_name);

            let mut assignable = Vec::new();
            let mut seen = Vec::new();
            for reference in type_def.directly_related_user_types(rel) {
                let key = (
                    reference.type_name.clone(),
                    reference.relation.clone(),
                    reference.wildcard.is_some(),
                );
                if seen.contains(&key) {
                    continue; // same subject with/without condition — one impl
                }
                seen.push(key);
                assignable.push(assignable_impl(
                    reference,
                    marker_ident,
                    names,
                    krate,
                    span,
                )?);
            }

            Ok(quote! {
                #[doc = #marker_doc]
                pub struct #marker_ident;
                #[doc = #const_doc]
                #[allow(non_upper_case_globals)]
                pub const #const_ident: #krate::typed::FgaRel<Ty, #marker_ident> =
                    #krate::typed::FgaRel::new(#rel);
                #(#assignable)*
            })
        })
        .collect::<syn::Result<Vec<_>>>()?;

    Ok(quote! {
        pub mod #mod_ident {
            #[doc = #ty_doc]
            pub struct Ty;
            impl #krate::typed::FgaType for Ty {
                const NAME: &'static str = #type_name;
            }

            #[doc = #id_doc]
            pub fn id(id: impl ::core::convert::AsRef<str>) -> #krate::typed::FgaObject<Ty> {
                #krate::typed::FgaObject::new(id)
            }

            /// Fallible form of [`id`] for request-supplied input.
            pub fn try_id(
                id: impl ::core::convert::AsRef<str>,
            ) -> ::core::result::Result<#krate::typed::FgaObject<Ty>, #krate::typed::InvalidObjectId> {
                #krate::typed::FgaObject::try_new(id)
            }

            #[doc = #wildcard_doc]
            pub fn wildcard() -> #krate::typed::FgaWildcard<Ty> {
                #krate::typed::FgaWildcard::new()
            }

            #(#relations)*
        }
    })
}

/// One `DirectlyAssignable` impl for a `directly_related_user_types` entry.
fn assignable_impl(
    reference: &RelationReference,
    marker_ident: &Ident,
    names: &ModuleNames,
    krate: &TokenStream,
    span: proc_macro2::Span,
) -> syn::Result<TokenStream> {
    // `validate()` guarantees the referenced type/relation exist.
    let subject_mod = names.types.get(&reference.type_name).ok_or_else(|| {
        syn::Error::new(
            span,
            format!("model!: unknown type `{}`", reference.type_name),
        )
    })?;

    let subject_marker = if reference.wildcard.is_some() {
        quote! { #krate::typed::WildcardOf<super::#subject_mod::Ty> }
    } else if let Some(rel) = &reference.relation {
        let (_, rel_marker) = names
            .relations
            .get(&(reference.type_name.clone(), rel.clone()))
            .ok_or_else(|| {
                syn::Error::new(
                    span,
                    format!("model!: unknown relation `{}#{}`", reference.type_name, rel),
                )
            })?;
        quote! { (super::#subject_mod::Ty, super::#subject_mod::#rel_marker) }
    } else {
        quote! { super::#subject_mod::Ty }
    };

    Ok(quote! {
        impl #krate::typed::DirectlyAssignable<#subject_marker> for #marker_ident {}
    })
}

/// `-` is legal in FGA identifiers but not in Rust ones.
fn sanitize(name: &str) -> String {
    name.replace('-', "_")
}

fn pascal_case(name: &str) -> String {
    name.split(['_', '-'])
        .filter(|s| !s.is_empty())
        .map(|seg| {
            let mut chars = seg.chars();
            match chars.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

/// Build an ident, falling back to a raw ident for Rust keywords.
fn make_ident(name: &str, span: proc_macro2::Span, what: &str) -> syn::Result<Ident> {
    syn::parse_str::<Ident>(name)
        .map(|i| Ident::new(&i.to_string(), span))
        .or_else(|_| {
            syn::parse_str::<Ident>(&format!("r#{}", name))
                .map(|_| format_ident!("r#{}", name, span = span))
        })
        .map_err(|_| {
            syn::Error::new(
                span,
                format!(
                    "model!: {} name `{}` cannot be used as a Rust identifier",
                    what, name
                ),
            )
        })
}
