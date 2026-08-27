//! Opaque-token backends: RFC 7662 introspection and OIDC `userinfo`.
//! The IdP side is a mini raw-TCP HTTP server (no extra dev-deps) that logs
//! every full raw request (start line, headers and body).

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use r2e_core::rt::io::{AsyncReadExt, AsyncWriteExt};
use r2e_core::rt::TcpListener;
use r2e_mcp::auth::{
    DiscoveryClient, IntrospectionBackend, McpAuthError, OAuthServerMetadata,
    TokenValidatorBackend, UserinfoBackend,
};
use serde_json::{json, Value};

const ISSUER: &str = "http://idp.test";
const RESOURCE: &str = "http://localhost:3000/mcp";

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// One-connection-at-a-time canned endpoint: answers each request with the
/// response `respond(path)` returns, and logs the FULL raw request
/// (content-length-framed, so POST bodies are captured even when they
/// arrive in a separate segment).
async fn mini_endpoint(
    respond: impl Fn(&str) -> (u16, Value) + Send + Sync + 'static,
) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let log = Arc::new(Mutex::new(Vec::new()));
    let requests = Arc::clone(&log);
    r2e_core::rt::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let mut buf = Vec::new();
            let mut tmp = [0u8; 4096];
            let request = loop {
                let n = match stream.read(&mut tmp).await {
                    Ok(0) | Err(_) => break String::from_utf8_lossy(&buf).into_owned(),
                    Ok(n) => n,
                };
                buf.extend_from_slice(&tmp[..n]);
                let text = String::from_utf8_lossy(&buf).into_owned();
                if let Some(header_end) = text.find("\r\n\r\n") {
                    let content_length = text
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())?
                        })
                        .unwrap_or(0);
                    if buf.len() >= header_end + 4 + content_length {
                        break text;
                    }
                }
            };
            let path = request.split_whitespace().nth(1).unwrap_or("/").to_string();
            requests.lock().unwrap().push(request);
            let (status, body) = respond(&path);
            let body = body.to_string();
            let reason = if status == 200 { "OK" } else { "Error" };
            let response = format!(
                "HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\n\
                 content-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.shutdown().await;
        }
    });
    (base, log)
}

/// Fixed (`discovery: off`-style) metadata carrying only the given opaque
/// endpoints.
fn fixed_discovery(introspection: Option<String>, userinfo: Option<String>) -> Arc<DiscoveryClient> {
    Arc::new(DiscoveryClient::fixed(OAuthServerMetadata::from_endpoints(
        ISSUER, None, None, None, None, userinfo, introspection,
    )))
}

fn introspection_backend(endpoint: &str) -> IntrospectionBackend {
    IntrospectionBackend::new(
        reqwest::Client::new(),
        fixed_discovery(None, None),
        "rs-client",
        "rs-secret",
    )
    .with_endpoint(endpoint)
}

fn active_response() -> Value {
    json!({
        "active": true,
        "sub": "alice",
        "scope": "mcp:read mcp:write",
        "aud": RESOURCE,
        "exp": unix_now() + 3600,
        "iss": ISSUER,
        "realm_access": { "roles": ["admin"] },
    })
}

// ── Introspection ──────────────────────────────────────────────────────────

#[tokio::test]
async fn introspection_accepts_an_active_token() {
    let (base, log) = mini_endpoint(|_| (200, active_response())).await;
    let backend = introspection_backend(&format!("{base}/introspect"))
        .with_audiences([RESOURCE.to_string()]);

    let principal = backend
        .validate("opaque-token-1")
        .await
        .expect("active token should be accepted");
    assert_eq!(principal.user.sub, "alice");
    assert!(principal.has_scope("mcp:read"));
    assert!(principal.has_scope("mcp:write"));
    assert!(principal.user.roles.contains(&"admin".to_string()));

    // RFC 7662 shape: POST, Basic client credentials, form-encoded token.
    let requests = log.lock().unwrap();
    let request = requests[0].to_ascii_lowercase();
    assert!(request.starts_with("post /introspect"), "request: {request}");
    assert!(
        request.contains("authorization: basic cnmty2xpzw50onjzlxnly3jlda=="),
        "missing Basic credentials: {request}"
    );
    assert!(request.contains("token=opaque-token-1"), "request: {request}");
    assert!(request.contains("token_type_hint=access_token"), "request: {request}");
}

#[tokio::test]
async fn introspection_caches_positive_results_per_token() {
    let (base, log) = mini_endpoint(|_| (200, active_response())).await;
    let backend = introspection_backend(&format!("{base}/introspect"));

    backend.validate("token-a").await.unwrap();
    backend.validate("token-a").await.unwrap();
    assert_eq!(log.lock().unwrap().len(), 1, "second call must hit the cache");

    backend.validate("token-b").await.unwrap();
    assert_eq!(log.lock().unwrap().len(), 2, "a different token is a cache miss");
}

#[tokio::test]
async fn introspection_rejects_and_negatively_caches_an_inactive_token() {
    let (base, log) = mini_endpoint(|_| (200, json!({ "active": false }))).await;
    let backend = introspection_backend(&format!("{base}/introspect"));

    for _ in 0..2 {
        let err = backend.validate("revoked").await.unwrap_err();
        assert!(
            matches!(err, McpAuthError::InvalidToken("token is not active")),
            "got {err:?}"
        );
    }
    assert_eq!(log.lock().unwrap().len(), 1, "the rejection must be cached");
}

#[tokio::test]
async fn introspection_rejects_a_wrong_audience() {
    let (base, _log) = mini_endpoint(|_| {
        let mut body = active_response();
        body["aud"] = json!("https://other-api.example.com");
        (200, body)
    })
    .await;
    let backend = introspection_backend(&format!("{base}/introspect"))
        .with_audiences([RESOURCE.to_string()]);

    let err = backend.validate("foreign").await.unwrap_err();
    assert!(
        matches!(err, McpAuthError::InvalidToken("token audience does not include this resource")),
        "got {err:?}"
    );
}

#[tokio::test]
async fn introspection_rejects_an_expired_token() {
    let (base, _log) = mini_endpoint(|_| {
        let mut body = active_response();
        body["exp"] = json!(unix_now() - 3600);
        (200, body)
    })
    .await;
    let backend = introspection_backend(&format!("{base}/introspect"));

    let err = backend.validate("stale").await.unwrap_err();
    assert!(matches!(err, McpAuthError::InvalidToken("token expired")), "got {err:?}");
}

#[tokio::test]
async fn introspection_rejects_an_issuer_mismatch() {
    let (base, _log) = mini_endpoint(|_| {
        let mut body = active_response();
        body["iss"] = json!("http://evil.test");
        (200, body)
    })
    .await;
    let backend = introspection_backend(&format!("{base}/introspect"));

    let err = backend.validate("foreign-iss").await.unwrap_err();
    assert!(
        matches!(err, McpAuthError::InvalidToken("token issuer mismatch")),
        "got {err:?}"
    );
}

#[tokio::test]
async fn introspection_maps_endpoint_failures_to_upstream_and_never_caches_them() {
    let (base, log) = mini_endpoint(|_| (500, json!({}))).await;
    let backend = introspection_backend(&format!("{base}/introspect"));

    for _ in 0..2 {
        let err = backend.validate("whatever").await.unwrap_err();
        assert!(matches!(err, McpAuthError::Upstream(_)), "got {err:?}");
    }
    assert_eq!(
        log.lock().unwrap().len(),
        2,
        "an IdP outage must not be cached — the next request retries"
    );
}

#[tokio::test]
async fn introspection_without_any_endpoint_names_the_config_key() {
    let backend = IntrospectionBackend::new(
        reqwest::Client::new(),
        fixed_discovery(None, None),
        "rs-client",
        "rs-secret",
    );
    let err = backend.validate("t").await.unwrap_err();
    match err {
        McpAuthError::Upstream(msg) => {
            assert!(msg.contains("mcp.auth.introspection-endpoint"), "got: {msg}")
        }
        other => panic!("expected Upstream, got {other:?}"),
    }
}

#[tokio::test]
async fn introspection_resolves_the_endpoint_from_discovery() {
    // No `with_endpoint`: the backend must use the advertised
    // `introspection_endpoint` (the mini endpoint sees the request path).
    let (base, log) = mini_endpoint(|_| (200, active_response())).await;
    let backend = IntrospectionBackend::new(
        reqwest::Client::new(),
        fixed_discovery(Some(format!("{base}/discovered-introspect")), None),
        "rs-client",
        "rs-secret",
    );
    backend.validate("token").await.unwrap();
    assert!(log.lock().unwrap()[0]
        .to_ascii_lowercase()
        .starts_with("post /discovered-introspect"));
}

#[tokio::test]
async fn positive_cache_is_capped_by_token_exp() {
    // `exp = now`: the token is still valid (leeway), but the cache entry
    // expires immediately — the second call must re-introspect.
    let (base, log) = mini_endpoint(|_| {
        let mut body = active_response();
        body["exp"] = json!(unix_now());
        (200, body)
    })
    .await;
    let backend = introspection_backend(&format!("{base}/introspect"))
        .with_cache(Duration::from_secs(3600), 16);

    backend.validate("edge").await.unwrap();
    backend.validate("edge").await.unwrap();
    assert_eq!(log.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn cache_entry_cap_evicts_when_full() {
    let (base, log) = mini_endpoint(|_| (200, active_response())).await;
    let backend = introspection_backend(&format!("{base}/introspect"))
        .with_cache(Duration::from_secs(3600), 1);

    backend.validate("token-a").await.unwrap();
    backend.validate("token-b").await.unwrap(); // evicts token-a (cap 1)
    backend.validate("token-a").await.unwrap(); // must re-introspect
    assert_eq!(log.lock().unwrap().len(), 3);
}

// ── Userinfo ───────────────────────────────────────────────────────────────

fn userinfo_backend(endpoint: &str) -> UserinfoBackend {
    UserinfoBackend::new(reqwest::Client::new(), fixed_discovery(None, None))
        .with_endpoint(endpoint)
}

#[tokio::test]
async fn userinfo_accepts_a_token_the_endpoint_accepts() {
    let (base, log) =
        mini_endpoint(|_| (200, json!({ "sub": "google-user", "email": "a@example.com" }))).await;
    let backend = userinfo_backend(&format!("{base}/userinfo"));

    let principal = backend.validate("ya29.opaque").await.expect("accepted token");
    assert_eq!(principal.user.sub, "google-user");
    assert_eq!(principal.user.email.as_deref(), Some("a@example.com"));

    let requests = log.lock().unwrap();
    let request = requests[0].to_ascii_lowercase();
    assert!(request.starts_with("get /userinfo"), "request: {request}");
    assert!(request.contains("authorization: bearer ya29.opaque"), "request: {request}");
}

#[tokio::test]
async fn userinfo_rejects_a_token_the_endpoint_rejects() {
    let (base, log) = mini_endpoint(|_| (401, json!({ "error": "invalid_token" }))).await;
    let backend = userinfo_backend(&format!("{base}/userinfo"));

    for _ in 0..2 {
        let err = backend.validate("expired").await.unwrap_err();
        assert!(
            matches!(err, McpAuthError::InvalidToken("token rejected by the userinfo endpoint")),
            "got {err:?}"
        );
    }
    assert_eq!(log.lock().unwrap().len(), 1, "the rejection must be cached");
}

#[tokio::test]
async fn userinfo_caches_positive_results_per_token() {
    let (base, log) = mini_endpoint(|_| (200, json!({ "sub": "google-user" }))).await;
    let backend = userinfo_backend(&format!("{base}/userinfo"));

    backend.validate("token").await.unwrap();
    backend.validate("token").await.unwrap();
    assert_eq!(log.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn userinfo_rejects_a_response_without_a_subject() {
    let (base, _log) = mini_endpoint(|_| (200, json!({ "email": "no-sub@example.com" }))).await;
    let backend = userinfo_backend(&format!("{base}/userinfo"));

    let err = backend.validate("odd").await.unwrap_err();
    assert!(
        matches!(err, McpAuthError::InvalidToken("userinfo response has no subject")),
        "got {err:?}"
    );
}

#[tokio::test]
async fn userinfo_maps_server_errors_to_upstream() {
    let (base, log) = mini_endpoint(|_| (503, json!({}))).await;
    let backend = userinfo_backend(&format!("{base}/userinfo"));

    for _ in 0..2 {
        let err = backend.validate("t").await.unwrap_err();
        assert!(matches!(err, McpAuthError::Upstream(_)), "got {err:?}");
    }
    assert_eq!(log.lock().unwrap().len(), 2, "outages are never cached");
}

#[tokio::test]
async fn userinfo_without_any_endpoint_names_the_config_key() {
    let backend = UserinfoBackend::new(reqwest::Client::new(), fixed_discovery(None, None));
    let err = backend.validate("t").await.unwrap_err();
    match err {
        McpAuthError::Upstream(msg) => {
            assert!(msg.contains("mcp.auth.userinfo-endpoint"), "got: {msg}")
        }
        other => panic!("expected Upstream, got {other:?}"),
    }
}
