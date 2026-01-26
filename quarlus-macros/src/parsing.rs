use syn::parse::{Parse, ParseStream};
use syn::Token;

use crate::route::{HttpMethod, RoutePath};

/// Parsed representation of the `controller!` macro input:
///
/// ```ignore
/// controller! {
///     impl HelloController for Services {
///         #[inject]
///         greeting: String,
///
///         #[get("/hello")]
///         async fn hello(&self) -> String { ... }
///     }
/// }
/// ```
pub struct ControllerDef {
    pub name: syn::Ident,
    pub state_type: syn::Path,
    pub injected_fields: Vec<InjectedField>,
    pub identity_fields: Vec<IdentityField>,
    pub route_methods: Vec<RouteMethod>,
    pub other_methods: Vec<syn::ImplItemFn>,
}

pub struct InjectedField {
    pub name: syn::Ident,
    pub ty: syn::Type,
}

pub struct IdentityField {
    pub name: syn::Ident,
    pub ty: syn::Type,
}

pub struct RouteMethod {
    pub method: HttpMethod,
    pub path: String,
    pub roles: Vec<String>,
    pub transactional: bool,
    pub fn_item: syn::ImplItemFn,
}

impl Parse for ControllerDef {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        // impl Name for StatePath { ... }
        let _: Token![impl] = input.parse()?;
        let name: syn::Ident = input.parse()?;
        let _: Token![for] = input.parse()?;
        let state_type: syn::Path = input.parse()?;

        let content;
        syn::braced!(content in input);

        let mut injected_fields = Vec::new();
        let mut identity_fields = Vec::new();
        let mut route_methods = Vec::new();
        let mut other_methods = Vec::new();

        while !content.is_empty() {
            let attrs = content.call(syn::Attribute::parse_outer)?;

            if is_method_ahead(&content) {
                let mut method: syn::ImplItemFn = content.parse()?;

                // Merge pre-parsed attrs with any attrs syn parsed
                let mut all_attrs = attrs;
                all_attrs.append(&mut method.attrs);

                match extract_route_attr(&all_attrs)? {
                    Some((http_method, path)) => {
                        let roles = extract_roles(&all_attrs)?;
                        let transactional = all_attrs.iter().any(|a| a.path().is_ident("transactional"));
                        method.attrs = strip_route_attrs(all_attrs);
                        route_methods.push(RouteMethod {
                            method: http_method,
                            path,
                            roles,
                            transactional,
                            fn_item: method,
                        });
                    }
                    None => {
                        method.attrs = all_attrs;
                        other_methods.push(method);
                    }
                }
            } else {
                // Field declaration: name: Type,
                let field_name: syn::Ident = content.parse()?;
                let _: Token![:] = content.parse()?;
                let field_type: syn::Type = content.parse()?;
                if content.peek(Token![,]) {
                    let _: Token![,] = content.parse()?;
                }

                let is_inject = attrs.iter().any(|a| a.path().is_ident("inject"));
                let is_identity = attrs.iter().any(|a| a.path().is_ident("identity"));

                if is_inject {
                    injected_fields.push(InjectedField {
                        name: field_name,
                        ty: field_type,
                    });
                } else if is_identity {
                    identity_fields.push(IdentityField {
                        name: field_name,
                        ty: field_type,
                    });
                } else {
                    return Err(syn::Error::new(
                        field_name.span(),
                        "field in controller must have #[inject] or #[identity]",
                    ));
                }
            }
        }

        Ok(ControllerDef {
            name,
            state_type,
            injected_fields,
            identity_fields,
            route_methods,
            other_methods,
        })
    }
}

fn is_method_ahead(input: ParseStream) -> bool {
    input.peek(Token![pub])
        || input.peek(Token![async])
        || input.peek(Token![fn])
        || input.peek(Token![unsafe])
}

fn is_route_attr(attr: &syn::Attribute) -> bool {
    attr.path().is_ident("get")
        || attr.path().is_ident("post")
        || attr.path().is_ident("put")
        || attr.path().is_ident("delete")
        || attr.path().is_ident("patch")
}

fn strip_route_attrs(attrs: Vec<syn::Attribute>) -> Vec<syn::Attribute> {
    attrs
        .into_iter()
        .filter(|a| !is_route_attr(a) && !a.path().is_ident("roles") && !a.path().is_ident("transactional"))
        .collect()
}

fn extract_roles(attrs: &[syn::Attribute]) -> syn::Result<Vec<String>> {
    for attr in attrs {
        if attr.path().is_ident("roles") {
            let args: syn::punctuated::Punctuated<syn::LitStr, syn::Token![,]> =
                attr.parse_args_with(syn::punctuated::Punctuated::parse_terminated)?;
            return Ok(args.iter().map(|lit| lit.value()).collect());
        }
    }
    Ok(Vec::new())
}

fn extract_route_attr(attrs: &[syn::Attribute]) -> syn::Result<Option<(HttpMethod, String)>> {
    for attr in attrs {
        let method = if attr.path().is_ident("get") {
            Some(HttpMethod::Get)
        } else if attr.path().is_ident("post") {
            Some(HttpMethod::Post)
        } else if attr.path().is_ident("put") {
            Some(HttpMethod::Put)
        } else if attr.path().is_ident("delete") {
            Some(HttpMethod::Delete)
        } else if attr.path().is_ident("patch") {
            Some(HttpMethod::Patch)
        } else {
            None
        };

        if let Some(method) = method {
            let route_path: RoutePath = attr.parse_args()?;
            return Ok(Some((method, route_path.path)));
        }
    }
    Ok(None)
}
