use std::time::Duration;

use r2e::config::R2eConfig;
use r2e::prelude::*;
use r2e::ws::WsStream;
use r2e_test::TestApp;

// ─── State ───

#[derive(Clone, TestState)]
struct WsTestState {
    config: R2eConfig,
}

// ─── Echo controller ───

#[derive(Controller)]
#[controller(path = "/ws", state = WsTestState)]
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
    let config = R2eConfig::empty();
    let state = WsTestState { config };
    TestApp::from_builder(
        AppBuilder::new()
            .with_state(state)
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
    let mut ws = server.ws("/ws/echo").await.with_timeout(Duration::from_millis(100));
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
