//! `DiscoveryClient`: probe order, issuer verification, cache semantics.
//! The IdP side is a mini raw-TCP HTTP server (no extra dev-deps) that logs
//! every requested path.

use std::sync::{Arc, Mutex};

use r2e_core::rt::io::{AsyncReadExt, AsyncWriteExt};
use r2e_core::rt::TcpListener;
use r2e_mcp::auth::{DiscoveryClient, McpAuthError, OAuthServerMetadata};
use serde_json::{json, Value};

/// One-connection-at-a-time canned IdP: answers each request path with the
/// response `respond` returns, and logs the path.
async fn mini_idp(
    respond: impl Fn(&str) -> (u16, Value) + Send + Sync + 'static,
) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let log = Arc::new(Mutex::new(Vec::new()));
    let paths = Arc::clone(&log);
    r2e_core::rt::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let mut buf = vec![0u8; 4096];
            let n = stream.read(&mut buf).await.unwrap_or(0);
            let request = String::from_utf8_lossy(&buf[..n]).into_owned();
            let path = request.split_whitespace().nth(1).unwrap_or("/").to_string();
            paths.lock().unwrap().push(path.clone());
            let (status, body) = respond(&path);
            let body = body.to_string();
            let reason = if status == 200 { "OK" } else { "Not Found" };
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

#[tokio::test]
async fn probes_openid_configuration_first_then_oauth_metadata() {
    // The document's `issuer` must echo the (bind-time) base URL — the
    // responder closes over a slot filled once the port is known.
    let issuer_slot = Arc::new(Mutex::new(String::new()));
    let issuer_in = Arc::clone(&issuer_slot);
    let (base, log) = mini_idp(move |path| {
        if path == "/.well-known/oauth-authorization-server" {
            (
                200,
                json!({ "issuer": *issuer_in.lock().unwrap(), "jwks_uri": "http://j/x" }),
            )
        } else {
            (404, json!({}))
        }
    })
    .await;
    *issuer_slot.lock().unwrap() = base.clone();

    let client = DiscoveryClient::new(reqwest::Client::new(), &base, 3600);
    let meta = client.get().await.expect("discovery should succeed");
    assert_eq!(meta.issuer, base);
    assert_eq!(meta.jwks_uri.as_deref(), Some("http://j/x"));
    // Probe order: openid-configuration first (404), then RFC 8414 metadata.
    let paths = log.lock().unwrap().clone();
    assert_eq!(
        paths,
        [
            "/.well-known/openid-configuration",
            "/.well-known/oauth-authorization-server"
        ]
    );
}

#[tokio::test]
async fn path_issuers_get_the_rfc8414_insertion_variants() {
    // An issuer with a path (Entra's `/tenant/v2.0` shape) where nothing
    // answers: all four candidates must be probed, in order.
    let (base, log) = mini_idp(|_| (404, json!({}))).await;
    let issuer = format!("{base}/tenant/v2.0");
    let client = DiscoveryClient::new(reqwest::Client::new(), &issuer, 3600);
    let err = client.get().await.unwrap_err();
    assert!(
        matches!(&err, McpAuthError::Upstream(m)
            if m.contains("OAuth discovery failed for issuer") && m.contains(&issuer)),
        "{err:?}"
    );
    let paths = log.lock().unwrap().clone();
    assert_eq!(
        paths,
        [
            "/tenant/v2.0/.well-known/openid-configuration",
            "/tenant/v2.0/.well-known/oauth-authorization-server",
            "/.well-known/oauth-authorization-server/tenant/v2.0",
            "/.well-known/openid-configuration/tenant/v2.0",
        ]
    );
}

#[tokio::test]
async fn issuer_mismatch_is_a_hard_error() {
    let (base, _log) = mini_idp(|_| (200, json!({ "issuer": "http://other.example" }))).await;
    let client = DiscoveryClient::new(reqwest::Client::new(), &base, 3600);
    let err = client.get().await.unwrap_err();
    assert!(
        matches!(&err, McpAuthError::Upstream(m)
            if m.contains("issuer mismatch")
                && m.contains(&base)
                && m.contains("http://other.example")),
        "{err:?}"
    );
}

#[tokio::test]
async fn fixed_metadata_never_fetches() {
    let meta = OAuthServerMetadata::from_endpoints(
        "http://127.0.0.1:1", // any fetch would fail instantly
        Some("http://127.0.0.1:1/jwks".into()),
        None,
        None,
        None,
        None,
    );
    let client = DiscoveryClient::fixed(meta);
    assert_eq!(client.issuer(), "http://127.0.0.1:1");
    let got = client
        .get()
        .await
        .expect("fixed metadata is always available");
    assert_eq!(got.jwks_uri.as_deref(), Some("http://127.0.0.1:1/jwks"));
}

#[tokio::test]
async fn stale_cache_survives_an_idp_outage() {
    // ttl 0 = immediately stale; the primed document must still be served
    // when the (dead) IdP cannot answer the refresh.
    let client = DiscoveryClient::new(reqwest::Client::new(), "http://127.0.0.1:1", 0);
    client
        .prime(OAuthServerMetadata::from_endpoints(
            "http://127.0.0.1:1",
            Some("http://127.0.0.1:1/jwks".into()),
            None,
            None,
            None,
            None,
        ))
        .await;
    let got = client.get().await.expect("stale-if-error");
    assert_eq!(got.issuer, "http://127.0.0.1:1");
}

#[tokio::test]
async fn dead_idp_with_no_cache_is_an_upstream_error() {
    let client = DiscoveryClient::new(reqwest::Client::new(), "http://127.0.0.1:1", 3600);
    let err = client.get().await.unwrap_err();
    assert!(
        matches!(&err, McpAuthError::Upstream(m)
            if m.contains("OAuth discovery failed for issuer `http://127.0.0.1:1`")),
        "{err:?}"
    );
}

#[tokio::test]
async fn metadata_without_issuer_is_rejected() {
    let err = OAuthServerMetadata::from_raw(json!({ "jwks_uri": "http://j/x" })).unwrap_err();
    assert!(
        matches!(&err, McpAuthError::Upstream(m)
            if m.contains("authorization server metadata has no `issuer`")),
        "{err:?}"
    );
}
