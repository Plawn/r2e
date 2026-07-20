use proc_macro2::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Fields, PathSegment, Type};

use crate::codegen::controller_impl::type_to_openapi_str;
use crate::crate_path::r2e_core_path;

pub fn expand(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = syn::parse_macro_input!(input as DeriveInput);
    match expand_inner(&input) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

fn expand_inner(input: &DeriveInput) -> syn::Result<TokenStream> {
    let name = &input.ident;
    let core = r2e_core_path();

    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(named) => &named.named,
            _ => {
                return Err(syn::Error::new_spanned(
                    input,
                    "FromMultipart can only be derived on structs with named fields",
                ))
            }
        },
        _ => {
            return Err(syn::Error::new_spanned(
                input,
                "FromMultipart can only be derived on structs",
            ))
        }
    };

    let mut field_extractions = Vec::new();
    let mut schema_properties = Vec::new();
    let mut schema_required = Vec::new();

    for field in fields {
        let field_name = field.ident.as_ref().unwrap();
        let field_name_str = field_name.to_string();
        let ty = &field.ty;

        // One classification drives both the runtime extraction and the
        // OpenAPI schema, so the two can never drift apart.
        let class = classify(ty)?;

        let extraction = extraction_tokens(&class, &field_name_str, &core);
        field_extractions.push(quote! {
            #field_name: #extraction
        });

        let (field_schema, required) = schema_tokens(&class);
        schema_properties.push(quote! { #field_name_str: #field_schema });
        if required {
            schema_required.push(quote! { #field_name_str });
        }
    }

    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    Ok(quote! {
        impl #impl_generics #core::multipart::FromMultipart for #name #ty_generics #where_clause {
            fn from_multipart(
                mut fields: #core::multipart::MultipartFields,
            ) -> ::std::result::Result<Self, #core::multipart::MultipartError> {
                ::std::result::Result::Ok(Self {
                    #(#field_extractions,)*
                })
            }
        }

        impl #impl_generics #core::meta::MultipartSchema for #name #ty_generics #where_clause {
            fn multipart_schema() -> #core::serde_json::Value {
                #core::serde_json::json!({
                    "type": "object",
                    "properties": { #(#schema_properties,)* },
                    "required": [ #(#schema_required,)* ],
                })
            }
        }
    })
}

/// How a multipart field is extracted and modeled.
enum FieldClass<'a> {
    /// `String` — required text field.
    Text,
    /// `UploadedFile` — required file.
    File,
    /// `Vec<UploadedFile>` — zero or more files (absent field = empty Vec).
    Files,
    /// `Bytes` — raw bytes from either a text or file field.
    Bytes,
    /// Any other type — text field parsed via `FromStr`.
    Parse(&'a Type),
    /// `Option<inner>` — inner extraction, absent field = `None`.
    Optional(Box<FieldClass<'a>>),
}

/// Classify a field type by its last path segment.
fn classify(ty: &Type) -> syn::Result<FieldClass<'_>> {
    match last_path_segment(ty).as_deref() {
        Some("Option") => {
            let inner = extract_generic_arg(ty)
                .ok_or_else(|| syn::Error::new_spanned(ty, "Option must have a type argument"))?;
            match classify(inner)? {
                FieldClass::Files => Err(syn::Error::new_spanned(
                    ty,
                    "FromMultipart: Option<Vec<UploadedFile>> is not supported — use \
                     Vec<UploadedFile>; an absent field yields an empty Vec",
                )),
                FieldClass::Optional(_) => Err(syn::Error::new_spanned(
                    ty,
                    "FromMultipart: nested Option is not supported",
                )),
                inner_class => Ok(FieldClass::Optional(Box::new(inner_class))),
            }
        }
        Some("Vec") => {
            let inner_seg = extract_generic_arg(ty).and_then(last_path_segment);
            match inner_seg.as_deref() {
                Some("UploadedFile") => Ok(FieldClass::Files),
                _ => Err(syn::Error::new_spanned(
                    ty,
                    "FromMultipart: Vec<T> is only supported for Vec<UploadedFile>",
                )),
            }
        }
        Some("UploadedFile") => Ok(FieldClass::File),
        Some("String") => Ok(FieldClass::Text),
        Some("Bytes") => Ok(FieldClass::Bytes),
        _ => Ok(FieldClass::Parse(ty)),
    }
}

/// Generate the extraction expression for a classified field.
fn extraction_tokens(class: &FieldClass, field_name: &str, core: &TokenStream) -> TokenStream {
    match class {
        FieldClass::Text => quote! { fields.take_text(#field_name)? },
        FieldClass::File => quote! { fields.take_file(#field_name)? },
        FieldClass::Files => quote! { fields.take_files(#field_name) },
        FieldClass::Bytes => quote! { fields.take_bytes(#field_name)? },
        FieldClass::Parse(_) => quote! {
            {
                let __val = fields.take_text(#field_name)?;
                __val.parse().map_err(|e| {
                    #core::multipart::MultipartError::ParseError {
                        field: #field_name.to_string(),
                        message: ::std::string::ToString::to_string(&e),
                    }
                })?
            }
        },
        FieldClass::Optional(inner) => match inner.as_ref() {
            FieldClass::Text => quote! { fields.take_text_opt(#field_name) },
            FieldClass::File => quote! { fields.take_file_opt(#field_name) },
            FieldClass::Bytes => quote! { fields.take_bytes(#field_name).ok() },
            FieldClass::Parse(_) => quote! {
                match fields.take_text_opt(#field_name) {
                    ::std::option::Option::Some(__val) => ::std::option::Option::Some(
                        __val.parse().map_err(|e| {
                            #core::multipart::MultipartError::ParseError {
                                field: #field_name.to_string(),
                                message: ::std::string::ToString::to_string(&e),
                            }
                        })?
                    ),
                    ::std::option::Option::None => ::std::option::Option::None,
                }
            },
            // Rejected by classify().
            FieldClass::Files | FieldClass::Optional(_) => unreachable!(),
        },
    }
}

/// JSON Schema fragment (as tokens usable inside `serde_json::json!`) plus
/// whether the field is listed in the schema's `required` array.
fn schema_tokens(class: &FieldClass) -> (TokenStream, bool) {
    match class {
        FieldClass::Text => (quote! { { "type": "string" } }, true),
        FieldClass::File | FieldClass::Bytes => {
            (quote! { { "type": "string", "format": "binary" } }, true)
        }
        // An absent field yields an empty Vec at runtime, so the schema must
        // not require it.
        FieldClass::Files => (
            quote! { { "type": "array", "items": { "type": "string", "format": "binary" } } },
            false,
        ),
        FieldClass::Parse(ty) => {
            let openapi_type = type_to_openapi_str(ty);
            (quote! { { "type": #openapi_type } }, true)
        }
        FieldClass::Optional(inner) => (schema_tokens(inner).0, false),
    }
}

/// Extract the last segment name from a type path (e.g. `Option` from `std::option::Option<T>`).
fn last_path_segment(ty: &Type) -> Option<String> {
    if let Type::Path(type_path) = ty {
        type_path
            .path
            .segments
            .last()
            .map(|seg| seg.ident.to_string())
    } else {
        None
    }
}

/// Extract the first generic argument from a type (e.g. `T` from `Option<T>` or `Vec<T>`).
fn extract_generic_arg(ty: &Type) -> Option<&Type> {
    if let Type::Path(type_path) = ty {
        let seg: &PathSegment = type_path.path.segments.last()?;
        if let syn::PathArguments::AngleBracketed(args) = &seg.arguments {
            if let Some(syn::GenericArgument::Type(inner)) = args.args.first() {
                return Some(inner);
            }
        }
    }
    None
}
