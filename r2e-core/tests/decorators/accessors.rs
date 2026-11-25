//! The small read accessors guards build on: `StandardClaims`' role and
//! scope views, the `Identity` defaults, the `'static` placeholders of a
//! hand-built `GuardContext`, and the request-head accessors of both guard
//! contexts.

use std::net::IpAddr;

use r2e_core::decorators::guards::{GuardContext, Identity, PathParams, PreAuthGuardContext};
use r2e_core::http::{HeaderMap, Method, Uri};
use r2e_core::StandardClaims;

fn claims(json: &str) -> StandardClaims {
    r2e_core::json::from_slice(json.as_bytes()).unwrap()
}

// ── StandardClaims ──────────────────────────────────────────────────────────

#[test]
fn realm_and_client_roles_read_the_keycloak_claims() {
    let claims = claims(
        r#"{
            "sub": "u",
            "realm_access": { "roles": ["admin", "user"] },
            "resource_access": {
                "billing": { "roles": ["invoice:read"] },
                "empty": { "roles": [] }
            }
        }"#,
    );
    assert_eq!(claims.realm_roles(), ["admin", "user"]);
    assert_eq!(claims.client_roles("billing"), ["invoice:read"]);
    assert!(claims.client_roles("empty").is_empty());
    assert!(claims.client_roles("unknown").is_empty());
}

#[test]
fn absent_keycloak_claims_read_as_no_roles() {
    let claims = claims(r#"{ "sub": "u" }"#);
    assert!(claims.realm_roles().is_empty());
    assert!(claims.client_roles("billing").is_empty());
}

#[test]
fn scopes_split_the_space_separated_claim() {
    let scoped = claims(r#"{ "sub": "u", "scope": "openid  docs:read\tdocs:write" }"#);
    assert_eq!(
        scoped.scopes().collect::<Vec<_>>(),
        ["openid", "docs:read", "docs:write"]
    );
    assert_eq!(claims(r#"{ "sub": "u" }"#).scopes().count(), 0);
}

// ── Identity defaults ───────────────────────────────────────────────────────

struct SubOnly;

impl Identity for SubOnly {
    fn sub(&self) -> &str {
        "sub-only"
    }
}

#[test]
fn an_identity_that_overrides_nothing_has_no_email_and_no_claims() {
    assert_eq!(SubOnly.email(), None);
    assert!(SubOnly.claims().is_none());
}

// ── 'static placeholders ────────────────────────────────────────────────────

#[test]
fn the_placeholders_are_get_and_empty_from_one_static_place() {
    assert_eq!(*r2e_core::default_method(), Method::GET);
    assert!(r2e_core::no_extensions().is_empty());
    // One place, not a fresh (leaked) value per call.
    assert!(std::ptr::eq(
        r2e_core::default_method(),
        r2e_core::default_method()
    ));
    assert!(std::ptr::eq(
        r2e_core::no_extensions(),
        r2e_core::no_extensions()
    ));
}

// ── request-head accessors ──────────────────────────────────────────────────

fn xff(value: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("x-forwarded-for", value.parse().unwrap());
    headers
}

fn guard_ctx<'a>(uri: &'a Uri, headers: &'a HeaderMap) -> GuardContext<'a, SubOnly> {
    GuardContext {
        method_name: "m",
        controller_name: "C",
        method: r2e_core::default_method(),
        extensions: r2e_core::no_extensions(),
        headers,
        uri,
        peer_addr: None,
        path_params: PathParams::EMPTY,
        identity: None,
    }
}

fn pre_auth_ctx<'a>(
    uri: &'a Uri,
    headers: &'a HeaderMap,
    path_params: PathParams<'a>,
) -> PreAuthGuardContext<'a> {
    PreAuthGuardContext {
        method_name: "m",
        controller_name: "C",
        headers,
        uri,
        peer_addr: None,
        path_params,
    }
}

#[test]
fn the_guard_context_reads_the_leftmost_forwarded_entry() {
    let uri: Uri = "/x".parse().unwrap();
    let headers = xff(" 203.0.113.9 , 10.0.0.1");
    let ctx = guard_ctx(&uri, &headers);
    assert_eq!(ctx.forwarded_for(), Some("203.0.113.9"));
    assert_eq!(
        ctx.forwarded_ip(),
        Some("203.0.113.9".parse::<IpAddr>().unwrap())
    );

    let none = HeaderMap::new();
    let ctx = guard_ctx(&uri, &none);
    assert_eq!(ctx.forwarded_for(), None);
    assert_eq!(ctx.forwarded_ip(), None);
}

#[test]
fn the_pre_auth_context_reads_path_query_and_params() {
    let uri: Uri = "/orgs/acme/docs?page=2&sort=asc".parse().unwrap();
    let headers = HeaderMap::new();
    let params = [("org", "acme")];
    let ctx = pre_auth_ctx(&uri, &headers, PathParams::from_pairs(&params));
    assert_eq!(ctx.path(), "/orgs/acme/docs");
    assert_eq!(ctx.query_string(), Some("page=2&sort=asc"));
    assert_eq!(ctx.path_param("org"), Some("acme"));
    assert_eq!(ctx.path_param("missing"), None);

    let bare: Uri = "/plain".parse().unwrap();
    let ctx = pre_auth_ctx(&bare, &headers, PathParams::EMPTY);
    assert_eq!(ctx.path(), "/plain");
    assert_eq!(ctx.query_string(), None);
    assert_eq!(ctx.path_param("org"), None);
}
