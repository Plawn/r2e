use bytes::Bytes;
use http::header::{HeaderMap, HeaderValue};
use http::StatusCode;
use r2e_test::{SameSite, TestResponse};

fn cookie_response(set_cookies: &[&str]) -> TestResponse {
    let mut headers = HeaderMap::new();
    for cookie in set_cookies {
        headers.append("set-cookie", HeaderValue::from_str(cookie).unwrap());
    }
    TestResponse::from_parts(StatusCode::OK, headers, Bytes::new())
}

#[test]
fn test_parse_basic_cookie() {
    let resp = cookie_response(&["session=abc123; Path=/; HttpOnly"]);
    let c = resp.set_cookie("session").unwrap();
    assert_eq!(c.name, "session");
    assert_eq!(c.value, "abc123");
    assert_eq!(c.path, Some("/".to_string()));
    assert!(c.http_only);
    assert!(!c.secure);
    assert!(c.same_site.is_none());
}

#[test]
fn test_parse_all_attributes() {
    let resp = cookie_response(&[
        "token=xyz; Path=/api; Domain=example.com; Max-Age=3600; Secure; HttpOnly; SameSite=Strict"
    ]);
    let c = resp.set_cookie("token").unwrap();
    assert_eq!(c.name, "token");
    assert_eq!(c.value, "xyz");
    assert_eq!(c.path, Some("/api".to_string()));
    assert_eq!(c.domain, Some("example.com".to_string()));
    assert_eq!(c.max_age, Some(3600));
    assert!(c.secure);
    assert!(c.http_only);
    assert_eq!(c.same_site, Some(SameSite::Strict));
}

#[test]
fn test_parse_same_site_variants() {
    for (attr, expected) in [
        ("SameSite=Strict", SameSite::Strict),
        ("SameSite=Lax", SameSite::Lax),
        ("SameSite=None", SameSite::None),
    ] {
        let resp = cookie_response(&[&format!("c=v; {attr}")]);
        let c = resp.set_cookie("c").unwrap();
        assert_eq!(c.same_site, Some(expected));
    }
}

#[test]
fn test_parse_max_age() {
    let resp = cookie_response(&["c=v; Max-Age=7200"]);
    let c = resp.set_cookie("c").unwrap();
    assert_eq!(c.max_age, Some(7200));
}

#[test]
fn test_set_cookies_returns_all() {
    let resp = cookie_response(&["a=1; Path=/", "b=2; Secure"]);
    let cookies = resp.set_cookies();
    assert_eq!(cookies.len(), 2);
    assert_eq!(cookies[0].name, "a");
    assert_eq!(cookies[1].name, "b");
}

#[test]
fn test_assert_cookie_secure() {
    let resp = cookie_response(&["token=abc; Secure"]);
    resp.assert_cookie_secure("token");
}

#[test]
fn test_assert_cookie_http_only() {
    let resp = cookie_response(&["session=abc; HttpOnly"]);
    resp.assert_cookie_http_only("session");
}

#[test]
fn test_assert_cookie_same_site() {
    let resp = cookie_response(&["c=v; SameSite=Lax"]);
    resp.assert_cookie_same_site("c", SameSite::Lax);
}

#[test]
fn test_assert_cookie_path() {
    let resp = cookie_response(&["c=v; Path=/api"]);
    resp.assert_cookie_path("c", "/api");
}

#[test]
fn test_missing_cookie_returns_none() {
    let resp = cookie_response(&["a=1"]);
    assert!(resp.set_cookie("nonexistent").is_none());
}
