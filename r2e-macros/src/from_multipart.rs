use proc_macro2::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Fields, Type, PathSegment};

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

        let extraction = classify_and_extract(ty, &field_name_str, &core)?;
        field_extractions.push(quote! {
            #field_name: #extraction
        });

        let (field_schema, required) = field_schema(ty);
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

        impl #impl_generics #core::multipart::MultipartSchema for #name #ty_generics #where_clause {
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

/// Map a field type to its JSON Schema fragment (as tokens usable inside
/// `serde_json::json!`) plus whether the field is required.
///
/// Mirrors the runtime classification in `classify_and_extract`: files and raw
/// bytes are `string`/`binary`, text fields map by primitive ident, and any
/// other `FromStr`-parsed type is modeled as a string.
fn field_schema(ty: &Type) -> (TokenStream, bool) {
    let last_seg = last_path_segment(ty);

    match last_seg.as_deref() {
        Some("Option") => {
            let inner_schema = extract_generic_arg(ty)
                .map(|inner| field_schema(inner).0)
                .unwrap_or_else(|| quote! { { "type": "string" } });
            (inner_schema, false)
        }
        // classify_and_extract only accepts Vec<UploadedFile>
        Some("Vec") => (
            quote! { { "type": "array", "items": { "type": "string", "format": "binary" } } },
            true,
        ),
        Some("UploadedFile") | Some("Bytes") => {
            (quote! { { "type": "string", "format": "binary" } }, true)
        }
        Some(
            "u8" | "u16" | "u32" | "u64" | "u128" | "usize" | "i8" | "i16" | "i32" | "i64"
            | "i128" | "isize",
        ) => (quote! { { "type": "integer" } }, true),
        Some("f32" | "f64") => (quote! { { "type": "number" } }, true),
        Some("bool") => (quote! { { "type": "boolean" } }, true),
        _ => (quote! { { "type": "string" } }, true),
    }
}

/// Classify a field type and generate the appropriate extraction code.
fn classify_and_extract(
    ty: &Type,
    field_name: &str,
    core: &TokenStream,
) -> syn::Result<TokenStream> {
    let last_seg = last_path_segment(ty);

    match last_seg.as_deref() {
        // Option<T> — inner extraction, MissingField → None
        Some("Option") => {
            let inner_ty = extract_generic_arg(ty)
                .ok_or_else(|| syn::Error::new_spanned(ty, "Option must have a type argument"))?;
            let inner_seg = last_path_segment(inner_ty);
            match inner_seg.as_deref() {
                Some("UploadedFile") => Ok(quote! {
                    fields.take_file_opt(#field_name)
                }),
                Some("String") => Ok(quote! {
                    fields.take_text_opt(#field_name)
                }),
                Some("Bytes") => Ok(quote! {
                    fields.take_bytes(#field_name).ok()
                }),
                _ => {
                    // Option<T> where T: FromStr
                    Ok(quote! {
                        match fields.take_text_opt(#field_name) {
                            ::std::option::Option::Some(v) => ::std::option::Option::Some(
                                v.parse().map_err(|e: Box<dyn ::std::fmt::Display>| {
                                    #core::multipart::MultipartError::ParseError {
                                        field: #field_name.to_string(),
                                        message: e.to_string(),
                                    }
                                })?
                            ),
                            ::std::option::Option::None => ::std::option::Option::None,
                        }
                    })
                }
            }
        }

        // Vec<UploadedFile>
        Some("Vec") => {
            let inner_ty = extract_generic_arg(ty);
            let inner_seg = inner_ty.and_then(|t| last_path_segment(t));
            match inner_seg.as_deref() {
                Some("UploadedFile") => Ok(quote! {
                    fields.take_files(#field_name)
                }),
                _ => {
                    Err(syn::Error::new_spanned(
                        ty,
                        "FromMultipart: Vec<T> is only supported for Vec<UploadedFile>",
                    ))
                }
            }
        }

        // UploadedFile — required file
        Some("UploadedFile") => Ok(quote! {
            fields.take_file(#field_name)?
        }),

        // String — required text
        Some("String") => Ok(quote! {
            fields.take_text(#field_name)?
        }),

        // Bytes — raw bytes
        Some("Bytes") => Ok(quote! {
            fields.take_bytes(#field_name)?
        }),

        // Anything else (i32, bool, f64, etc.) — text then parse
        _ => Ok(quote! {
            {
                let __val = fields.take_text(#field_name)?;
                __val.parse().map_err(|e| {
                    #core::multipart::MultipartError::ParseError {
                        field: #field_name.to_string(),
                        message: ::std::string::ToString::to_string(&e),
                    }
                })?
            }
        }),
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
