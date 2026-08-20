use opentelemetry::trace::TracerProvider;
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::trace::SdkTracerProvider;
use r2e_observability::{inject_current_context, traced_reqwest_client};
use tracing_subscriber::layer::SubscriberExt;

#[test]
fn injects_the_current_span_context() {
    opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());
    let provider = SdkTracerProvider::builder().build();
    let tracer = provider.tracer("client-test");
    let subscriber = tracing_subscriber::Registry::default()
        .with(tracing_opentelemetry::layer().with_tracer(tracer));
    let subscriber_guard = tracing::subscriber::set_default(subscriber);

    let span = tracing::info_span!("outgoing call");
    let span_guard = span.enter();
    let mut headers = reqwest::header::HeaderMap::new();
    inject_current_context(&mut headers);

    let traceparent = headers["traceparent"].to_str().unwrap();
    let parts: Vec<_> = traceparent.split('-').collect();
    assert_eq!(parts.len(), 4);
    assert_eq!(parts[1].len(), 32);
    assert_eq!(parts[2].len(), 16);

    drop(span_guard);
    drop(subscriber_guard);
    provider.shutdown().unwrap();
}

#[test]
fn injection_is_a_noop_without_an_otel_span() {
    opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());
    let mut headers = reqwest::header::HeaderMap::new();
    inject_current_context(&mut headers);
    assert!(!headers.contains_key("traceparent"));

    let _client = traced_reqwest_client(reqwest::Client::builder().build().unwrap());
}

/// One-shot HTTP/1 server: accepts a single connection, returns the raw
/// request head, answers `200 OK`.
async fn one_shot_server() -> (std::net::SocketAddr, tokio::task::JoinHandle<String>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 8192];
        let mut head = String::new();
        loop {
            let n = socket.read(&mut buf).await.unwrap();
            head.push_str(&String::from_utf8_lossy(&buf[..n]));
            if n == 0 || head.contains("\r\n\r\n") {
                break;
            }
        }
        socket
            .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
            .await
            .unwrap();
        socket.shutdown().await.ok();
        head
    });
    (addr, handle)
}

fn header_value<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines().find_map(|line| {
        let (k, v) = line.split_once(':')?;
        k.eq_ignore_ascii_case(name).then(|| v.trim())
    })
}

#[tokio::test]
async fn middleware_opens_a_client_span_and_propagates_its_id() {
    use opentelemetry::trace::SpanKind;
    use opentelemetry_sdk::trace::InMemorySpanExporter;
    use tracing::Instrument;

    opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());
    let exporter = InMemorySpanExporter::default();
    let provider = SdkTracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let tracer = provider.tracer("client-test");
    let subscriber = tracing_subscriber::Registry::default()
        .with(tracing_opentelemetry::layer().with_tracer(tracer));
    let subscriber_guard = tracing::subscriber::set_default(subscriber);

    let (addr, server) = one_shot_server().await;
    let client = traced_reqwest_client(reqwest::Client::builder().build().unwrap());
    let url = format!("http://user:secret@{addr}/items/42?q=1");

    let parent = tracing::info_span!("caller", otel.kind = "server");
    let response = client.get(&url).send().instrument(parent).await.unwrap();
    assert_eq!(response.status(), 200);
    let head = server.await.unwrap();

    drop(subscriber_guard);
    provider.force_flush().unwrap();
    let spans = exporter.get_finished_spans().unwrap();
    assert_eq!(spans.len(), 2, "expected parent + client spans, got {spans:#?}");

    let parent_span = spans.iter().find(|s| s.name == "caller").unwrap();
    let client_span = spans.iter().find(|s| s.name != "caller").unwrap();

    // Semconv HTTP client span, child of the caller.
    assert_eq!(client_span.span_kind, SpanKind::Client);
    assert_eq!(client_span.name, "HTTP GET");
    assert_eq!(client_span.parent_span_id, parent_span.span_context.span_id());
    let attr = |key: &str| {
        client_span
            .attributes
            .iter()
            .find(|kv| kv.key.as_str() == key)
            .map(|kv| kv.value.to_string())
    };
    assert_eq!(attr("http.request.method").as_deref(), Some("GET"));
    assert_eq!(attr("server.address").as_deref(), Some("127.0.0.1"));
    assert_eq!(attr("server.port").as_deref(), Some(addr.port().to_string().as_str()));
    assert_eq!(attr("http.response.status_code").as_deref(), Some("200"));
    assert_eq!(
        attr("url.full").as_deref(),
        Some(format!("http://{addr}/items/42?q=1").as_str()),
        "credentials must be stripped from url.full"
    );

    // The propagated traceparent carries the CLIENT span id, not the parent's.
    let traceparent = header_value(&head, "traceparent").expect("traceparent header");
    let parts: Vec<_> = traceparent.split('-').collect();
    assert_eq!(parts.len(), 4);
    assert_eq!(parts[1], client_span.span_context.trace_id().to_string());
    assert_eq!(parts[2], client_span.span_context.span_id().to_string());
    assert_ne!(parts[2], parent_span.span_context.span_id().to_string());

    provider.shutdown().unwrap();
}

#[tokio::test]
async fn middleware_records_failures_on_the_client_span() {
    use opentelemetry::trace::{SpanKind, Status};
    use opentelemetry_sdk::trace::InMemorySpanExporter;

    let exporter = InMemorySpanExporter::default();
    let provider = SdkTracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let tracer = provider.tracer("client-test");
    let subscriber = tracing_subscriber::Registry::default()
        .with(tracing_opentelemetry::layer().with_tracer(tracer));
    let subscriber_guard = tracing::subscriber::set_default(subscriber);

    // Bind then drop: nothing listens on this port anymore.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);

    let client = traced_reqwest_client(reqwest::Client::builder().build().unwrap());
    let err = client.post(format!("http://{addr}/")).send().await.unwrap_err();
    assert!(err.is_connect(), "{err}");

    drop(subscriber_guard);
    provider.force_flush().unwrap();
    let spans = exporter.get_finished_spans().unwrap();
    assert_eq!(spans.len(), 1);
    let span = &spans[0];
    assert_eq!(span.span_kind, SpanKind::Client);
    assert_eq!(span.name, "HTTP POST");
    assert!(matches!(span.status, Status::Error { .. }), "{:?}", span.status);
    assert!(span.attributes.iter().any(|kv| kv.key.as_str() == "error.message"));

    provider.shutdown().unwrap();
}
