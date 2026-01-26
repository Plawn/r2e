use syn::parse::{Parse, ParseStream};
use syn::LitStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
    Patch,
}

impl HttpMethod {
    pub fn as_axum_method_fn(&self) -> &'static str {
        match self {
            HttpMethod::Get => "get",
            HttpMethod::Post => "post",
            HttpMethod::Put => "put",
            HttpMethod::Delete => "delete",
            HttpMethod::Patch => "patch",
        }
    }
}

/// Parse le path depuis les arguments d'un attribut : `("/users/{id}")`
pub struct RoutePath {
    pub path: String,
}

impl Parse for RoutePath {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let lit: LitStr = input.parse()?;
        Ok(RoutePath { path: lit.value() })
    }
}
