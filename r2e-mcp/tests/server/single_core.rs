//! One endpoint type, one runtime core (#1215): a type whose `#[routes]` impl
//! mixes HTTP routes and MCP members is both a `Controller` and an
//! `McpService`, and `register_controller` / `register_mcp_service` share the
//! builder's one core for it — in either registration order. So a tool sees
//! what a `#[post_construct]` hook wrote, calls a `#[consumer]` method through
//! the interceptors `register_controller` filled, and runs behind the same
//! struct identity and impl-level guard as the HTTP route. A type declaring
//! controller-hosted members but registered only as an MCP service fails the
//! build with an actionable message.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

use r2e_core::http::extract::FromRequestParts;
use r2e_core::http::header::Parts;
use r2e_core::http::{Body, Request, Routes, StatusCode};
use r2e_core::prelude::*;
use r2e_core::{AppBuilder, Identity, Rejection, RejectionKind};
use r2e_events::LocalEventBus;
use r2e_mcp::{AppBuilderMcpExt, McpServer};
use serde_json::{json, Value};
use tower::ServiceExt;

use crate::support;

/// Which transport registers first.
#[derive(Clone, Copy, Debug)]
enum Order {
    ControllerFirst,
    McpFirst,
}

const ORDERS: [Order; 2] = [Order::ControllerFirst, Order::McpFirst];

async fn http_get(router: &Routes, path: &str, headers: &[(&str, &str)]) -> StatusCode {
    let mut request = Request::builder().method("GET").uri(path);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = router
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    response.status()
}

fn text(message: &Value) -> &str {
    message["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("no text content in {message}"))
}

// ── T1: `#[post_construct]` state is visible to a tool ──────────────────────

/// Core-local state: every `ctx.get::<CoreSlot>()` clones the provided (empty)
/// bean, so each constructed core owns its own `OnceLock`. A second core —
/// the MCP side constructing its own — would read it empty.
#[derive(Clone, Default)]
pub struct CoreSlot(OnceLock<&'static str>);

#[controller(path = "/lifecycle")]
pub struct Lifecycle {
    #[inject]
    slot: CoreSlot,
}

#[routes]
impl Lifecycle {
    #[post_construct]
    async fn init(&self) {
        let _ = self.slot.0.set("ready");
    }

    #[get("/state")]
    async fn state(&self) -> String {
        self.read_slot()
    }

    #[tool]
    async fn state_tool(&self) -> String {
        self.read_slot()
    }

    fn read_slot(&self) -> String {
        self.slot.0.get().copied().unwrap_or("unset").to_string()
    }
}

async fn lifecycle_app(order: Order) -> Routes {
    let builder = AppBuilder::new()
        .plugin(McpServer::new())
        .provide(CoreSlot::default())
        .build_state()
        .await;
    let builder = match order {
        Order::ControllerFirst => builder
            .register_controller::<Lifecycle>()
            .register_mcp_service::<Lifecycle>(),
        Order::McpFirst => builder
            .register_mcp_service::<Lifecycle>()
            .register_controller::<Lifecycle>(),
    };
    builder.build_with_consumers().await
}

#[r2e_core::test]
async fn a_tool_sees_what_post_construct_wrote_in_either_order() {
    for order in ORDERS {
        let router = lifecycle_app(order).await;
        let session = support::initialize(&router, "/mcp").await;
        let message = support::tools_call(&router, "/mcp", &session, "state_tool", json!({})).await;
        assert_eq!(text(&message), "ready", "{order:?}: {message}");
    }
}

// ── T2: an intercepted `#[consumer]` called from a tool ─────────────────────

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Ping;

/// Interceptor calls, shared by every core (a bean): the count is only
/// non-zero when the called core's decorator slot was filled.
#[derive(Clone, Default)]
pub struct Calls(Arc<AtomicUsize>);

#[derive(DecoratorBean)]
pub struct Counting {
    #[inject]
    calls: Calls,
}

impl<R: Send> Interceptor<R> for Counting {
    fn around<F, Fut>(
        &self,
        _ctx: InterceptorContext,
        next: F,
    ) -> impl std::future::Future<Output = R> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: std::future::Future<Output = R> + Send,
    {
        self.calls.0.fetch_add(1, Ordering::SeqCst);
        next()
    }
}

#[controller(path = "/pings")]
pub struct Pings {
    #[inject]
    event_bus: LocalEventBus,
}

#[routes]
impl Pings {
    #[get("/")]
    async fn ping_route(&self) -> &'static str {
        "pong"
    }

    #[consumer(bus = "event_bus")]
    #[intercept(Counting::spec())]
    async fn on_ping(&self, _event: Arc<Ping>) {}

    /// Calls the consumer method directly: the call goes through the core's
    /// decorator slot, which only `register_controller` fills.
    #[tool]
    async fn fire(&self) -> String {
        self.on_ping(Arc::new(Ping)).await;
        "fired".to_string()
    }
}

#[r2e_core::test]
async fn a_tool_calls_an_intercepted_consumer_through_the_filled_slot_in_either_order() {
    for order in ORDERS {
        let calls = Calls::default();
        let builder = AppBuilder::new()
            .plugin(McpServer::new())
            .provide(LocalEventBus::new())
            .provide(calls.clone())
            .build_state()
            .await;
        let builder = match order {
            Order::ControllerFirst => builder
                .register_controller::<Pings>()
                .register_mcp_service::<Pings>(),
            Order::McpFirst => builder
                .register_mcp_service::<Pings>()
                .register_controller::<Pings>(),
        };
        let router = builder.build_with_consumers().await;
        let session = support::initialize(&router, "/mcp").await;
        let message = support::tools_call(&router, "/mcp", &session, "fire", json!({})).await;
        assert_eq!(text(&message), "fired", "{order:?}: {message}");
        assert_eq!(calls.0.load(Ordering::SeqCst), 1, "{order:?}");
    }
}

// ── T3: struct identity + impl-level guard on both transports ───────────────

/// The `x-user` header, as an identity.
#[derive(Clone)]
pub struct Caller(String);

impl Identity for Caller {
    fn sub(&self) -> &str {
        &self.0
    }
}

impl<S: Send + Sync> FromRequestParts<S> for Caller {
    type Rejection = Rejection;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .headers
            .get("x-user")
            .and_then(|value| value.to_str().ok())
            .map(|user| Caller(user.to_owned()))
            .ok_or_else(|| Rejection::new(RejectionKind::Unauthenticated, "missing x-user"))
    }
}

/// Admits `alice` only.
pub struct OnlyAlice;

impl SelfBuilt for OnlyAlice {}

impl<I: Identity> Guard<I> for OnlyAlice {
    async fn check(&self, ctx: &GuardContext<'_, I>) -> Result<(), Rejection> {
        match ctx.identity {
            Some(identity) if identity.sub() == "alice" => Ok(()),
            _ => Err(Rejection::forbidden("only alice")),
        }
    }
}

#[controller(path = "/guarded")]
pub struct Guarded {
    #[inject(identity)]
    caller: Caller,
}

#[routes]
#[guard(OnlyAlice)]
impl Guarded {
    #[get("/")]
    async fn whoami(&self) -> String {
        self.caller.0.clone()
    }

    #[tool]
    async fn whoami_tool(&self) -> String {
        self.caller.0.clone()
    }
}

async fn guarded_app() -> Routes {
    AppBuilder::new()
        .plugin(McpServer::new())
        .build_state()
        .await
        .register_controller::<Guarded>()
        .register_mcp_service::<Guarded>()
        .build()
}

#[r2e_core::test]
async fn the_struct_identity_and_the_impl_guard_gate_both_transports() {
    let router = guarded_app().await;
    let alice = [("x-user", "alice")];
    let bob = [("x-user", "bob")];

    // No credentials: the identity rejects both transports — the HTTP route
    // and every request on the MCP endpoint (here a `tools/call` on a
    // session alice opened).
    assert_eq!(
        http_get(&router, "/guarded", &[]).await,
        StatusCode::UNAUTHORIZED
    );
    let session = support::initialize_with_headers(&router, "/mcp", &alice).await;
    let response = support::post(
        &router,
        "/mcp",
        Some(&session),
        &json!({
            "jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": { "name": "whoami_tool", "arguments": {} }
        }),
    )
    .await;
    assert_eq!(
        response.status,
        StatusCode::UNAUTHORIZED,
        "{}",
        response.raw_body
    );

    // Authenticated but refused by the impl-level guard: both transports.
    assert_eq!(
        http_get(&router, "/guarded", &bob).await,
        StatusCode::FORBIDDEN
    );
    let bob_session = support::initialize_with_headers(&router, "/mcp", &bob).await;
    let denied = support::tools_call_with_headers(
        &router,
        "/mcp",
        &bob_session,
        &bob,
        "whoami_tool",
        json!({}),
    )
    .await;
    assert!(denied.get("result").is_none(), "{denied}");
    assert_eq!(denied["error"]["data"], "forbidden", "{denied}");

    // Authenticated and admitted: both transports succeed.
    assert_eq!(http_get(&router, "/guarded", &alice).await, StatusCode::OK);
    let admitted = support::tools_call_with_headers(
        &router,
        "/mcp",
        &session,
        &alice,
        "whoami_tool",
        json!({}),
    )
    .await;
    assert_eq!(text(&admitted), "alice", "{admitted}");
}

// ── T4: controller-hosted members need `register_controller` ────────────────

#[r2e_core::test]
#[should_panic(
    expected = "declares #[post_construct] members but was only registered with \
                `register_mcp_service`; those members are wired by `register_controller` \
                alone — add `.register_controller::<Lifecycle>()`"
)]
async fn lifecycle_members_registered_only_as_an_mcp_service_fail_the_build() {
    let _router = AppBuilder::new()
        .plugin(McpServer::new())
        .provide(CoreSlot::default())
        .build_state()
        .await
        .register_mcp_service::<Lifecycle>()
        .build();
}

#[r2e_core::test]
async fn lifecycle_members_registered_on_both_transports_boot() {
    for order in ORDERS {
        let router = lifecycle_app(order).await;
        assert_eq!(
            http_get(&router, "/lifecycle/state", &[]).await,
            StatusCode::OK,
            "{order:?}"
        );
    }
}
