//! `#[module(prefix = "…")]` is validated at the declaration: the prefix ends
//! up in `Router::nest` and in every published OpenAPI path, so a missing
//! leading slash would silently produce routes nobody declared.

use r2e::prelude::*;

#[derive(Clone)]
pub struct Svc;

#[bean]
impl Svc {
    fn new() -> Self {
        Self
    }
}

#[module(prefix = "api/v1", providers(Svc))]
pub struct NoLeadingSlash;

#[module(prefix = "/api/v1/", providers(Svc))]
pub struct TrailingSlash;

#[module(prefix = "/tenants/{id}", providers(Svc))]
pub struct Parameterized;

fn main() {}
