use crate::route::HttpMethod;

pub struct InjectedField {
    pub name: syn::Ident,
    pub ty: syn::Type,
}

pub struct IdentityField {
    pub name: syn::Ident,
    pub ty: syn::Type,
}

pub struct ConfigField {
    pub name: syn::Ident,
    pub ty: syn::Type,
    pub key: String,
}

pub struct ConsumerMethod {
    pub bus_field: String,
    pub event_type: syn::Type,
    pub fn_item: syn::ImplItemFn,
}

pub struct ScheduledConfig {
    pub every: Option<u64>,
    pub cron: Option<String>,
    pub initial_delay: Option<u64>,
    pub name: Option<String>,
}

pub struct ScheduledMethod {
    pub config: ScheduledConfig,
    pub intercept_fns: Vec<syn::Expr>,
    pub fn_item: syn::ImplItemFn,
}

pub struct IdentityParam {
    pub index: usize,
    pub ty: syn::Type,
}

pub struct RouteMethod {
    pub method: HttpMethod,
    pub path: String,
    pub roles: Vec<String>,
    pub transactional: Option<TransactionalConfig>,
    pub intercept_fns: Vec<syn::Expr>,
    pub guard_fns: Vec<syn::Expr>,
    pub middleware_fns: Vec<syn::Path>,
    pub layer_exprs: Vec<syn::Expr>,
    pub identity_param: Option<IdentityParam>,
    pub fn_item: syn::ImplItemFn,
}

pub struct TransactionalConfig {
    pub pool_field: String,
}
