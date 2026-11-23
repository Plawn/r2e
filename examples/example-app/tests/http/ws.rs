use std::time::Duration;

use r2e::prelude::*;
use r2e::web::ws::WsStream;
use r2e_test::TestApp;

// ─── Echo controller ───

#[controller(path = "/ws")]
pub struct WsEchoTestController;

#[routes]
impl WsEchoTestController {
    #[ws("/echo")]
    async fn echo(&self, mut ws: WsStream) {
        ws.send_text("welcome").await.ok();
        ws.on_each(|msg| async move { Some(msg) }).await;
    }
}

async fn setup() -> TestApp {
    TestApp::from_builder(
        AppBuilder::new()
            .build_state()
            .await
            .register_controller::<WsEchoTestController>(),
    )
}

#[r2e::test]
async fn test_ws_connect_and_receive_welcome() {
    let app = setup().await;
    let server = app.serve().await;
    let mut ws = server.ws("/ws/echo").await;

    let msg = ws.next_text().await.expect("should receive welcome");
    assert_eq!(msg, "welcome");
}

#[r2e::test]
async fn test_ws_echo_text() {
    let app = setup().await;
    let server = app.serve().await;
    let mut ws = server.ws("/ws/echo").await;

    // Consume the welcome message
    ws.next_text().await.unwrap();

    ws.send_text("hello").await.unwrap();
    let reply = ws.next_text().await.unwrap();
    assert_eq!(reply, "hello");
}

#[r2e::test]
async fn test_ws_echo_json() {
    let app = setup().await;
    let server = app.serve().await;
    let mut ws = server.ws("/ws/echo").await;
    ws.next_text().await.unwrap(); // consume welcome

    ws.send_json(&serde_json::json!({"key": "value"}))
        .await
        .unwrap();
    let reply: serde_json::Value = ws.next_json().await.unwrap();
    assert_eq!(reply, serde_json::json!({"key": "value"}));
}

#[r2e::test]
async fn test_ws_timeout_on_no_message() {
    let app = setup().await;
    let server = app.serve().await;
    let mut ws = server
        .ws("/ws/echo")
        .await
        .with_timeout(Duration::from_millis(100));
    ws.next_text().await.unwrap(); // consume welcome

    // No message sent — next_text should timeout
    let result = ws.next_text().await;
    assert!(result.is_err());
}

#[r2e::test]
async fn test_ws_assert_no_message() {
    let app = setup().await;
    let server = app.serve().await;
    let mut ws = server.ws("/ws/echo").await;
    ws.next_text().await.unwrap(); // consume welcome

    ws.assert_no_message(Duration::from_millis(50)).await;
}

#[r2e::test]
async fn test_ws_multiple_echoes() {
    let app = setup().await;
    let server = app.serve().await;
    let mut ws = server.ws("/ws/echo").await;
    ws.next_text().await.unwrap(); // consume welcome

    for i in 0..5 {
        let msg = format!("msg-{i}");
        ws.send_text(&msg).await.unwrap();
        let reply = ws.next_text().await.unwrap();
        assert_eq!(reply, msg);
    }
}

// ─── Subprotocols (`Sec-WebSocket-Protocol`) and refusal before the upgrade ───
//
// The browser pattern: a page can set no header on a WebSocket but
// `Sec-WebSocket-Protocol`, so the credential travels there, next to the real
// subprotocol — `collab.v1, app.ticket.<ticket>`. The ticket is judged by a
// guard before the upgrade; the upgrade response echoes the subprotocol alone.

mod subprotocols {
    use r2e::prelude::*;
    use r2e::web::ws::{requested_protocols, WsStream};
    use r2e_test::{TestApp, WsTestClient};

    const V1: &str = "collab.v1";
    const V2: &str = "collab.v2";

    /// The app's error envelope: `{error, detail}` instead of `{"error": msg}`.
    #[derive(Debug, serde::Serialize)]
    struct Envelope {
        #[serde(skip)]
        status: StatusCode,
        error: String,
        detail: String,
    }

    impl From<Rejection> for Envelope {
        fn from(r: Rejection) -> Self {
            Self {
                status: r.status,
                error: format!("{:?}", r.kind),
                detail: r.message.into_owned(),
            }
        }
    }

    impl IntoHttpResponse for Envelope {
        fn into_http_response(self) -> Response {
            (self.status, Json(self)).into_response()
        }
    }
    r2e::http::impl_into_response!(Envelope);

    impl ErrorSchema for Envelope {
        fn body_schema() -> Option<(String, serde_json::Value)> {
            None
        }
    }

    /// Judges the ticket carried as a subprotocol — before identity, before
    /// the upgrade.
    pub struct TicketGate;
    impl SelfBuilt for TicketGate {}
    impl PreAuthGuard for TicketGate {
        async fn check(&self, ctx: &PreAuthGuardContext<'_>) -> Result<(), Rejection> {
            let ticket = requested_protocols(ctx.headers)
                .find_map(|protocol| protocol.strip_prefix("app.ticket."));
            match ticket {
                Some("good") => Ok(()),
                Some(_) => Err(Rejection::forbidden("ticket refused")),
                None => Err(Rejection::forbidden("ticket missing")),
            }
        }
    }

    #[controller(path = "/collab")]
    pub struct CollabController;

    #[routes]
    impl CollabController {
        /// Reports what was negotiated, then ends.
        #[ws("/open", protocols = [V2, "collab.v1"])]
        async fn open(&self, mut ws: WsStream) {
            let negotiated = ws.protocol().unwrap_or("none").to_string();
            ws.send_text(negotiated).await.ok();
        }

        #[ws("/ticketed", protocols = [V1])]
        #[pre_guard(TicketGate)]
        async fn ticketed(&self, mut ws: WsStream) {
            let negotiated = ws.protocol().unwrap_or("none").to_string();
            ws.send_text(negotiated).await.ok();
        }

        #[ws("/plain")]
        async fn plain(&self, mut ws: WsStream) {
            let negotiated = ws.protocol().unwrap_or("none").to_string();
            ws.send_text(negotiated).await.ok();
        }
    }

    async fn setup() -> TestApp {
        TestApp::from_builder(
            AppBuilder::new()
                .error_projection::<Envelope>()
                .build_state()
                .await
                .register_controller::<CollabController>(),
        )
    }

    #[r2e::test]
    async fn route_preference_wins_over_client_order() {
        let app = setup().await;
        let server = app.serve().await;
        // The client lists v1 first; the route prefers v2.
        let mut ws = server.ws_with_protocols("/collab/open", &[V1, V2]).await;
        assert_eq!(ws.protocol(), Some(V2), "echoed in the upgrade response");
        assert_eq!(ws.next_text().await.unwrap(), V2, "read by the handler");
    }

    #[r2e::test]
    async fn only_offered_protocols_are_selected() {
        let app = setup().await;
        let server = app.serve().await;
        let mut ws = server.ws_with_protocols("/collab/open", &[V1]).await;
        assert_eq!(ws.protocol(), Some(V1));
        assert_eq!(ws.next_text().await.unwrap(), V1);
    }

    #[r2e::test]
    async fn client_offering_nothing_gets_no_subprotocol() {
        let app = setup().await;
        let server = app.serve().await;
        let mut ws = server.ws("/collab/open").await;
        assert_eq!(ws.protocol(), None);
        assert_eq!(ws.next_text().await.unwrap(), "none");
    }

    /// The server does not refuse an offer it cannot honour: it upgrades with
    /// no `Sec-WebSocket-Protocol`, and it is the client that fails the
    /// connection — a browser does, and so does the test client.
    #[r2e::test]
    async fn unmatched_offer_is_failed_by_the_client_not_refused_by_the_server() {
        let app = setup().await;
        let server = app.serve().await;

        for path in ["/collab/open", "/collab/plain"] {
            let url = format!("{}{path}", server.ws_url());
            let error = WsTestClient::connect_with_protocols(&url, &["other"])
                .await
                .err()
                .expect("the client fails a handshake that selected no subprotocol");
            assert_eq!(error.http_status(), None, "{path}: upgraded, not refused: {error}");
        }
    }

    #[r2e::test]
    async fn ticket_is_judged_before_the_upgrade_and_never_echoed() {
        let app = setup().await;
        let server = app.serve().await;
        let mut ws = server
            .ws_with_protocols("/collab/ticketed", &[V1, "app.ticket.good"])
            .await;
        assert_eq!(ws.protocol(), Some(V1), "the subprotocol, without the ticket");
        assert_eq!(ws.next_text().await.unwrap(), V1);
    }

    #[r2e::test]
    async fn refused_ticket_is_an_http_response_in_the_app_envelope() {
        let app = setup().await;

        // In-process: the guard runs before the upgrade extractor, so the
        // refusal is observable without a live socket.
        let resp = app
            .get("/collab/ticketed")
            .header("connection", "upgrade")
            .header("upgrade", "websocket")
            .header("sec-websocket-version", "13")
            .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
            .header("sec-websocket-protocol", "collab.v1, app.ticket.bad")
            .send()
            .await;
        resp.assert_forbidden();
        assert_eq!(resp.header("sec-websocket-protocol"), None);
        let body: serde_json::Value = resp.json();
        assert_eq!(
            body,
            serde_json::json!({ "error": "Forbidden", "detail": "ticket refused" })
        );

        // Over a live connection: the handshake fails with that HTTP response.
        let server = app.serve().await;
        let url = format!("{}/collab/ticketed", server.ws_url());
        let refused = WsTestClient::connect_with_protocols(&url, &[V1]).await;
        let error = refused.err().expect("the upgrade must be refused without a ticket");
        assert_eq!(error.http_status(), Some(StatusCode::FORBIDDEN), "{error}");
    }
}

mod limits {
    use r2e::prelude::*;
    use r2e::web::ws::WsStream;
    use r2e_test::TestApp;

    const VOICE_FRAME: usize = 64;

    #[controller(path = "/limits")]
    pub struct LimitedController {
        #[config("collab.max-message-size")]
        collab_max_message_size: usize,
    }

    #[routes]
    impl LimitedController {
        /// The bound read from `[collab]` at each upgrade.
        fn collab_limit(&self) -> usize {
            self.collab_max_message_size
        }

        #[ws("/literal", max_message_size = 1024)]
        async fn literal(&self, mut ws: WsStream) {
            ws.on_each(|msg| async move { Some(msg) }).await;
        }

        #[ws("/configured", max_message_size = "collab_limit")]
        async fn configured(&self, mut ws: WsStream) {
            ws.on_each(|msg| async move { Some(msg) }).await;
        }

        #[ws("/frames", max_frame_size = VOICE_FRAME)]
        async fn frames(&self, mut ws: WsStream) {
            ws.on_each(|msg| async move { Some(msg) }).await;
        }

        #[ws(
            "/negotiated",
            protocols = ["collab.v1"],
            max_message_size = "collab_limit",
            max_frame_size = 4 * 1024
        )]
        async fn negotiated(&self, mut ws: WsStream) {
            let negotiated = ws.protocol().unwrap_or("none").to_string();
            ws.send_text(negotiated).await.ok();
            ws.on_each(|msg| async move { Some(msg) }).await;
        }

        #[ws("/unbounded")]
        async fn unbounded(&self, mut ws: WsStream) {
            ws.on_each(|msg| async move { Some(msg) }).await;
        }
    }

    async fn setup(collab_limit: usize) -> TestApp {
        let config = R2eConfig::from_yaml_str(&format!(
            "collab:\n  max-message-size: {collab_limit}\n"
        ))
        .unwrap();
        TestApp::from_builder(
            AppBuilder::new()
                .override_config(config)
                .load_config::<()>()
                .build_state()
                .await
                .register_controller::<LimitedController>(),
        )
    }

    #[r2e::test]
    async fn message_within_the_bound_is_delivered() {
        let app = setup(2048).await;
        let server = app.serve().await;
        let mut ws = server.ws("/limits/literal").await;
        let payload = "a".repeat(1024);
        ws.send_text(payload.clone()).await.unwrap();
        assert_eq!(ws.next_text().await.unwrap(), payload);
    }

    #[r2e::test]
    async fn message_over_a_literal_bound_closes_the_connection() {
        let app = setup(2048).await;
        let server = app.serve().await;
        let mut ws = server.ws("/limits/literal").await;
        ws.send_text("a".repeat(1025)).await.unwrap();
        ws.assert_closed().await;
    }

    #[r2e::test]
    async fn bound_is_read_from_config_at_each_upgrade() {
        // Same route, two configurations: the bound follows `[collab]`.
        for (limit, sent, delivered) in [(2048, 2000, true), (512, 2000, false), (512, 512, true)] {
            let app = setup(limit).await;
            let server = app.serve().await;
            let mut ws = server.ws("/limits/configured").await;
            let payload = "a".repeat(sent);
            ws.send_text(payload.clone()).await.unwrap();
            if delivered {
                assert_eq!(ws.next_text().await.unwrap(), payload, "limit {limit}, sent {sent}");
            } else {
                ws.assert_closed().await;
            }
        }
    }

    #[r2e::test]
    async fn frame_over_the_frame_bound_closes_the_connection() {
        let app = setup(2048).await;
        let server = app.serve().await;

        let mut ws = server.ws("/limits/frames").await;
        ws.send_binary(vec![7u8; VOICE_FRAME]).await.unwrap();
        assert_eq!(ws.next_binary().await.unwrap(), vec![7u8; VOICE_FRAME]);

        let mut ws = server.ws("/limits/frames").await;
        ws.send_binary(vec![7u8; VOICE_FRAME + 1]).await.unwrap();
        ws.assert_closed().await;
    }

    #[r2e::test]
    async fn limits_combine_with_protocols() {
        let app = setup(8192).await;
        let server = app.serve().await;

        let mut ws = server.ws_with_protocols("/limits/negotiated", &["collab.v1"]).await;
        assert_eq!(ws.protocol(), Some("collab.v1"));
        assert_eq!(ws.next_text().await.unwrap(), "collab.v1");
        let payload = "a".repeat(4096);
        ws.send_text(payload.clone()).await.unwrap();
        assert_eq!(ws.next_text().await.unwrap(), payload);

        // Under the message bound (8192) but over the frame bound (4096).
        let mut ws = server.ws_with_protocols("/limits/negotiated", &["collab.v1"]).await;
        ws.next_text().await.unwrap();
        ws.send_text("a".repeat(4097)).await.unwrap();
        ws.assert_closed().await;
    }

    #[r2e::test]
    async fn route_without_limits_keeps_the_defaults() {
        let app = setup(16).await;
        let server = app.serve().await;
        let mut ws = server.ws("/limits/unbounded").await;
        let payload = "a".repeat(1 << 20);
        ws.send_text(payload.clone()).await.unwrap();
        assert_eq!(ws.next_text().await.unwrap().len(), payload.len());
    }
}
