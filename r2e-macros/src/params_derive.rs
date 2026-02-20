use proc_macro2::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Fields, Ident, LitStr, Type};

use crate::crate_path::r2e_core_path;

enum ParamSource {
    Path { name: String },
    Query { name: String },
    Header { name: String },
}

struct ParamField {
    ident: Ident,
    ty: Type,
    source: ParamSource,
    is_optional: bool,
}

pub fn expand(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = syn::parse_macro_input!(input as DeriveInput);
    match expand_inner(input) {
        Ok(ts) => ts.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

fn expand_inner(input: DeriveInput) -> syn::Result<TokenStream> {
    let krate = r2e_core_path();
    let name = &input.ident;
    let (_impl_generics, ty_generics, _where_clause) = input.generics.split_for_impl();

    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(f) => &f.named,
            _ => {
                return Err(syn::Error::new_spanned(
                    name,
                    "Params can only be derived for structs with named fields",
                ))
            }
        },
        _ => {
            return Err(syn::Error::new_spanned(
                name,
                "Params can only be derived for structs",
            ))
        }
    };

    let mut param_fields = Vec::new();

    for field in fields {
        let ident = field.ident.clone().unwrap();
        let ty = field.ty.clone();
        let is_optional = is_option_type(&ty);

        let mut source = None;

        for attr in &field.attrs {
            if attr.path().is_ident("path") {
                let custom_name = parse_name_attr(attr)?;
                let name = custom_name.unwrap_or_else(|| ident.to_string());
                source = Some(ParamSource::Path { name });
            } else if attr.path().is_ident("query") {
                let custom_name = parse_name_attr(attr)?;
                let name = custom_name.unwrap_or_else(|| ident.to_string());
                source = Some(ParamSource::Query { name });
            } else if attr.path().is_ident("header") {
                // #[header("X-Custom-Header")]
                let header_name: LitStr = attr.parse_args()?;
                source = Some(ParamSource::Header {
                    name: header_name.value(),
                });
            }
        }

        if let Some(source) = source {
            param_fields.push(ParamField {
                ident,
                ty,
                source,
                is_optional,
            });
        }
        // Fields without path/query/header are ignored (may be garde fields or other)
    }

    // Group fields by source type
    let has_path_fields = param_fields
        .iter()
        .any(|f| matches!(f.source, ParamSource::Path { .. }));
    let has_query_fields = param_fields
        .iter()
        .any(|f| matches!(f.source, ParamSource::Query { .. }));

    // Generate extraction code for path params
    let path_extraction = if has_path_fields {
        quote! {
            let __raw_path = <#krate::http::extract::RawPathParams as #krate::http::extract::FromRequestParts<__R2eParamsState>>::from_request_parts(parts, _state)
                .await
                .map_err(|e| {
                    let err = #krate::params::ParamError {
                        message: format!("Failed to extract path parameters: {}", e),
                    };
                    #krate::http::response::IntoResponse::into_response(err)
                })?;
        }
    } else {
        quote! {}
    };

    // Generate extraction code for query params
    let query_extraction = if has_query_fields {
        quote! {
            let __query_pairs = #krate::params::parse_query_string(parts.uri.query());
        }
    } else {
        quote! {}
    };

    // Generate field construction
    let field_constructions: Vec<TokenStream> = param_fields
        .iter()
        .map(|f| generate_field_construction(f, &krate))
        .collect();

    // Collect all field idents for struct construction
    let all_field_idents: Vec<_> = param_fields.iter().map(|f| &f.ident).collect();

    let expanded = quote! {
        const _: () = {
            impl<__R2eParamsState: Send + Sync> #krate::http::extract::FromRequestParts<__R2eParamsState> for #name #ty_generics {
                type Rejection = #krate::http::response::Response;

                async fn from_request_parts(
                    parts: &mut #krate::http::header::Parts,
                    _state: &__R2eParamsState,
                ) -> Result<Self, Self::Rejection> {
                    use #krate::http::response::IntoResponse as _;

                    #path_extraction
                    #query_extraction

                    #(#field_constructions)*

                    Ok(Self {
                        #(#all_field_idents,)*
                    })
                }
            }
        };
    };

    Ok(expanded)
}

fn generate_field_construction(field: &ParamField, krate: &TokenStream) -> TokenStream {
    let ident = &field.ident;
    let name_str = match &field.source {
        ParamSource::Path { name } => name.as_str(),
        ParamSource::Query { name } => name.as_str(),
        ParamSource::Header { name } => name.as_str(),
    };

    match &field.source {
        ParamSource::Path { .. } => {
            if field.is_optional {
                let inner_ty = unwrap_option_type(&field.ty).unwrap();
                quote! {
                    let #ident: Option<#inner_ty> = match __raw_path.iter().find(|(k, _)| k.as_str() == #name_str) {
                        Some((_, v)) => {
                            match v.parse() {
                                Ok(val) => Some(val),
                                Err(_) => return Err(#krate::http::response::IntoResponse::into_response(
                                    #krate::params::ParamError {
                                        message: format!("Invalid path parameter '{}': parse error", #name_str),
                                    }
                                )),
                            }
                        }
                        None => None,
                    };
                }
            } else {
                quote! {
                    let #ident = match __raw_path.iter().find(|(k, _)| k.as_str() == #name_str) {
                        Some((_, v)) => v.parse().map_err(|_| #krate::http::response::IntoResponse::into_response(
                            #krate::params::ParamError {
                                message: format!("Invalid path parameter '{}': parse error", #name_str),
                            }
                        ))?,
                        None => return Err(#krate::http::response::IntoResponse::into_response(
                            #krate::params::ParamError {
                                message: format!("Missing path parameter '{}'", #name_str),
                            }
                        )),
                    };
                }
            }
        }
        ParamSource::Query { .. } => {
            if field.is_optional {
                let inner_ty = unwrap_option_type(&field.ty).unwrap();
                quote! {
                    let #ident: Option<#inner_ty> = match __query_pairs.iter().find(|(k, _)| k == #name_str) {
                        Some((_, v)) => Some(v.parse().map_err(|_| #krate::http::response::IntoResponse::into_response(
                            #krate::params::ParamError {
                                message: format!("Invalid query parameter '{}': parse error", #name_str),
                            }
                        ))?),
                        None => None,
                    };
                }
            } else {
                quote! {
                    let #ident = match __query_pairs.iter().find(|(k, _)| k == #name_str) {
                        Some((_, v)) => v.parse().map_err(|_| #krate::http::response::IntoResponse::into_response(
                            #krate::params::ParamError {
                                message: format!("Invalid query parameter '{}': parse error", #name_str),
                            }
                        ))?,
                        None => return Err(#krate::http::response::IntoResponse::into_response(
                            #krate::params::ParamError {
                                message: format!("Missing query parameter '{}'", #name_str),
                            }
                        )),
                    };
                }
            }
        }
        ParamSource::Header { .. } => {
            if field.is_optional {
                let inner_ty = unwrap_option_type(&field.ty).unwrap();
                quote! {
                    let #ident: Option<#inner_ty> = match parts.headers.get(#name_str) {
                        Some(v) => {
                            let s = v.to_str().map_err(|_| #krate::http::response::IntoResponse::into_response(
                                #krate::params::ParamError {
                                    message: format!("Invalid header '{}': not valid UTF-8", #name_str),
                                }
                            ))?;
                            Some(s.parse().map_err(|_| #krate::http::response::IntoResponse::into_response(
                                #krate::params::ParamError {
                                    message: format!("Invalid header '{}': parse error", #name_str),
                                }
                            ))?)
                        }
                        None => None,
                    };
                }
            } else {
                quote! {
                    let #ident = match parts.headers.get(#name_str) {
                        Some(v) => {
                            let s = v.to_str().map_err(|_| #krate::http::response::IntoResponse::into_response(
                                #krate::params::ParamError {
                                    message: format!("Invalid header '{}': not valid UTF-8", #name_str),
                                }
                            ))?;
                            s.parse().map_err(|_| #krate::http::response::IntoResponse::into_response(
                                #krate::params::ParamError {
                                    message: format!("Invalid header '{}': parse error", #name_str),
                                }
                            ))?
                        }
                        None => return Err(#krate::http::response::IntoResponse::into_response(
                            #krate::params::ParamError {
                                message: format!("Missing required header '{}'", #name_str),
                            }
                        )),
                    };
                }
            }
        }
    }
}

fn parse_name_attr(attr: &syn::Attribute) -> syn::Result<Option<String>> {
    // #[path] or #[path(name = "custom")]
    // #[query] or #[query(name = "custom")]
    match attr.meta {
        syn::Meta::Path(_) => Ok(None),
        syn::Meta::List(_) => {
            let mut name = None;
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("name") {
                    let value = meta.value()?;
                    let lit: LitStr = value.parse()?;
                    name = Some(lit.value());
                    Ok(())
                } else {
                    Err(meta.error("expected `name = \"...\"`"))
                }
            })?;
            Ok(name)
        }
        _ => Ok(None),
    }
}

fn is_option_type(ty: &Type) -> bool {
    unwrap_option_type(ty).is_some()
}

fn unwrap_option_type(ty: &Type) -> Option<&Type> {
    if let Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            if segment.ident == "Option" {
                if let syn::PathArguments::AngleBracketed(ref args) = segment.arguments {
                    if let Some(syn::GenericArgument::Type(inner)) = args.args.first() {
                        return Some(inner);
                    }
                }
            }
        }
    }
    None
}
