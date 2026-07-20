#![cfg(feature = "quic")]

use std::sync::Arc;

use r2e_http::quic;

fn generate_test_certs() -> (Vec<u8>, Vec<u8>) {
    let certified = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let cert_pem = certified.cert.pem().into_bytes();
    let key_pem = certified.signing_key.serialize_pem().into_bytes();
    (cert_pem, key_pem)
}

fn make_client_endpoint(cert_pem: &[u8], alpn: &[&[u8]]) -> quinn::Endpoint {
    let certs: Vec<_> = rustls_pemfile::certs(&mut &*cert_pem)
        .collect::<Result<_, _>>()
        .unwrap();
    let mut roots = rustls::RootCertStore::empty();
    for cert in &certs {
        roots.add(cert.clone()).unwrap();
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut tls_config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    tls_config.alpn_protocols = alpn.iter().map(|a| a.to_vec()).collect();
    let quic_config = quinn::crypto::rustls::QuicClientConfig::try_from(tls_config).unwrap();
    let client_config = quinn::ClientConfig::new(Arc::new(quic_config));
    let mut endpoint = quinn::Endpoint::client("0.0.0.0:0".parse().unwrap()).unwrap();
    endpoint.set_default_client_config(client_config);
    endpoint
}

#[tokio::test]
async fn serve_h3_round_trip() {
    let _ = tracing_subscriber::fmt::try_init();

    let (cert_pem, key_pem) = generate_test_certs();
    let server_config = quic::build_server_config(&cert_pem, &key_pem).unwrap();

    let router =
        r2e_http::Router::new().route("/ping", r2e_http::routing::get(|| async { "pong" }));

    let endpoint = quinn::Endpoint::server(server_config, "127.0.0.1:0".parse().unwrap()).unwrap();
    let server_addr = endpoint.local_addr().unwrap();

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

    let server_handle = tokio::spawn(async move {
        quic::serve_h3_with_endpoint(router, endpoint, async {
            shutdown_rx.await.ok();
        })
        .await
        .unwrap();
    });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let client = make_client_endpoint(&cert_pem, &[b"h3"]);
    let conn = client
        .connect(server_addr, "localhost")
        .unwrap()
        .await
        .unwrap();
    let h3_conn = h3_quinn::Connection::new(conn);
    let (mut driver, mut send_request) = h3::client::new(h3_conn).await.unwrap();

    let driver_handle = tokio::spawn(async move {
        let _ = std::future::poll_fn(|cx| driver.poll_close(cx)).await;
    });

    let req = http::Request::get("https://localhost/ping")
        .body(())
        .unwrap();
    let mut resp_stream = send_request.send_request(req).await.unwrap();
    resp_stream.finish().await.unwrap();

    let resp = resp_stream.recv_response().await.unwrap();
    assert_eq!(resp.status(), http::StatusCode::OK);

    let mut body = Vec::new();
    while let Some(chunk) = resp_stream.recv_data().await.unwrap() {
        use bytes::Buf;
        body.extend_from_slice(chunk.chunk());
    }
    assert_eq!(body, b"pong");

    let _ = shutdown_tx.send(());
    drop(driver_handle);
    client.close(0u32.into(), b"done");
    server_handle.await.unwrap();
}

#[tokio::test]
async fn serve_h3_post_body_streaming() {
    let _ = tracing_subscriber::fmt::try_init();

    let (cert_pem, key_pem) = generate_test_certs();
    let server_config = quic::build_server_config(&cert_pem, &key_pem).unwrap();

    let router = r2e_http::Router::new().route(
        "/echo",
        r2e_http::routing::post(|body: r2e_http::Bytes| async move { body }),
    );

    let endpoint = quinn::Endpoint::server(server_config, "127.0.0.1:0".parse().unwrap()).unwrap();
    let server_addr = endpoint.local_addr().unwrap();

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

    let server_handle = tokio::spawn(async move {
        quic::serve_h3_with_endpoint(router, endpoint, async {
            shutdown_rx.await.ok();
        })
        .await
        .unwrap();
    });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let client = make_client_endpoint(&cert_pem, &[b"h3"]);
    let conn = client
        .connect(server_addr, "localhost")
        .unwrap()
        .await
        .unwrap();
    let h3_conn = h3_quinn::Connection::new(conn);
    let (mut driver, mut send_request) = h3::client::new(h3_conn).await.unwrap();

    let driver_handle = tokio::spawn(async move {
        let _ = std::future::poll_fn(|cx| driver.poll_close(cx)).await;
    });

    let payload = b"hello from the client body";
    let req = http::Request::post("https://localhost/echo")
        .body(())
        .unwrap();
    let mut resp_stream = send_request.send_request(req).await.unwrap();
    resp_stream
        .send_data(bytes::Bytes::from_static(payload))
        .await
        .unwrap();
    resp_stream.finish().await.unwrap();

    let resp = resp_stream.recv_response().await.unwrap();
    assert_eq!(resp.status(), http::StatusCode::OK);

    let mut body = Vec::new();
    while let Some(chunk) = resp_stream.recv_data().await.unwrap() {
        use bytes::Buf;
        body.extend_from_slice(chunk.chunk());
    }
    assert_eq!(body, payload);

    let _ = shutdown_tx.send(());
    drop(driver_handle);
    client.close(0u32.into(), b"done");
    server_handle.await.unwrap();
}

#[tokio::test]
async fn serve_h3_payload_too_large() {
    let _ = tracing_subscriber::fmt::try_init();

    let (cert_pem, key_pem) = generate_test_certs();
    let server_config = quic::build_server_config(&cert_pem, &key_pem).unwrap();

    let router = r2e_http::Router::new().route(
        "/upload",
        r2e_http::routing::post(|body: r2e_http::Bytes| async move { body }),
    );

    let endpoint = quinn::Endpoint::server(server_config, "127.0.0.1:0".parse().unwrap()).unwrap();
    let server_addr = endpoint.local_addr().unwrap();

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

    let server_handle = tokio::spawn(async move {
        quic::serve_h3_with_endpoint(router, endpoint, async {
            shutdown_rx.await.ok();
        })
        .await
        .unwrap();
    });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let client = make_client_endpoint(&cert_pem, &[b"h3"]);
    let conn = client
        .connect(server_addr, "localhost")
        .unwrap()
        .await
        .unwrap();
    let h3_conn = h3_quinn::Connection::new(conn);
    let (mut driver, mut send_request) = h3::client::new(h3_conn).await.unwrap();

    let driver_handle = tokio::spawn(async move {
        let _ = std::future::poll_fn(|cx| driver.poll_close(cx)).await;
    });

    // Send Content-Length that exceeds DEFAULT_MAX_BODY_SIZE (2 MiB).
    // The server should reject with 413 via the Content-Length pre-check.
    let req = http::Request::post("https://localhost/upload")
        .header(
            "content-length",
            (quic::DEFAULT_MAX_BODY_SIZE + 1).to_string(),
        )
        .body(())
        .unwrap();
    let mut resp_stream = send_request.send_request(req).await.unwrap();
    resp_stream.finish().await.unwrap();

    let resp = resp_stream.recv_response().await.unwrap();
    assert_eq!(resp.status(), http::StatusCode::PAYLOAD_TOO_LARGE);

    let _ = shutdown_tx.send(());
    drop(driver_handle);
    client.close(0u32.into(), b"done");
    server_handle.await.unwrap();
}

#[tokio::test]
async fn raw_quic_bidirectional_stream() {
    let _ = tracing_subscriber::fmt::try_init();

    let (cert_pem, key_pem) = generate_test_certs();
    let server_config =
        quic::build_server_config_with_alpn(&cert_pem, &key_pem, vec![b"test-proto".to_vec()])
            .unwrap();

    let endpoint = quic::QuicEndpoint::bind("127.0.0.1:0".parse().unwrap(), server_config).unwrap();
    let server_addr = endpoint.local_addr().unwrap();

    let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();

    let server_handle = tokio::spawn(async move {
        let incoming = endpoint.accept().await.unwrap();
        let conn = quic::QuicConnection::new(incoming.await.unwrap());
        let (mut send, mut recv) = conn.accept_bi().await.unwrap();

        let mut buf = vec![0u8; 64];
        let n = recv.read(&mut buf).await.unwrap().unwrap();
        send.write_all(&buf[..n]).await.unwrap();
        send.finish().unwrap();

        // Wait for client to finish reading before closing
        let _ = done_rx.await;
        endpoint.close(b"done");
        endpoint.wait_idle().await;
    });

    let client = make_client_endpoint(&cert_pem, &[b"test-proto"]);
    let conn = client
        .connect(server_addr, "localhost")
        .unwrap()
        .await
        .unwrap();

    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    send.write_all(b"hello quic").await.unwrap();
    send.finish().unwrap();

    let response = recv.read_to_end(1024).await.unwrap();
    assert_eq!(response, b"hello quic");

    let _ = done_tx.send(());
    conn.close(0u32.into(), b"done");
    client.close(0u32.into(), b"done");
    server_handle.await.unwrap();
}
