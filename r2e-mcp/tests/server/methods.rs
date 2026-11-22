//! Extension JSON-RPC methods (#1444): `#[method(name = "…")]` members of a
//! `#[routes]` service, and `McpServer::with_extension`.
//!
//! * a method is served like a tool — same guards, roles, scopes and
//!   identity, same `McpError` table — over stateful HTTP, `mcp.stateless`
//!   and the loopback transport;
//! * an unknown method stays `-32601`;
//! * the methods rmcp models itself (`skills/list`, `skills/get`) are
//!   bridged to the same routes and gated on their extension;
//! * declared extensions are advertised by `initialize` and
//!   `server/discover`;
//! * a method that cannot be served aborts startup.

use std::sync::Arc;

use r2e_core::http::{Routes, StatusCode};
use r2e_core::prelude::*;
use r2e_core::AppBuilder;
use r2e_mcp::{AppBuilderMcpExt, McpError, McpPrincipal, McpServer, MethodCall, Params};
use r2e_security::AuthenticatedUser;
use r2e_test::TestJwt;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::support;

const SKILLS: &str = "io.modelcontextprotocol/skills";
const APPS: &str = "io.modelcontextprotocol/ui";
const SKILL_URI: &str = "skill://acme/review/SKILL.md";

// ── Fixtures ───────────────────────────────────────────────────────────────

#[derive(Deserialize, JsonSchema)]
struct GetSkill {
    uri: String,
}

#[derive(Deserialize, JsonSchema)]
struct Render {
    template: String,
    #[serde(default)]
    width: u32,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Rendered {
    html: String,
    width: u32,
}

fn review_skill() -> Value {
    json!({
        "uri": SKILL_URI,
        "frontmatter": { "name": "review", "description": "Review a pull request" },
        "resources": "dynamic"
    })
}

#[controller]
struct SkillMethods;

#[routes]
impl SkillMethods {
    #[method(name = "skills/list")]
    async fn list(&self) -> Value {
        json!({ "skills": [review_skill()] })
    }

    #[method(name = "skills/get")]
    async fn get(&self, Params(p): Params<GetSkill>) -> Result<Value, McpError> {
        if p.uri == SKILL_URI {
            Ok(json!({ "skill": review_skill() }))
        } else {
            Err(McpError::NotFound(format!("no skill at {}", p.uri)))
        }
    }
}

#[controller]
struct AppMethods;

#[routes]
impl AppMethods {
    /// Typed parameters, typed result.
    #[method(name = "ui/render")]
    async fn render(&self, Params(p): Params<Render>) -> Json<Rendered> {
        Json(Rendered {
            html: format!("<{}/>", p.template),
            width: p.width,
        })
    }

    /// The raw call: method name, params, session.
    #[method(name = "ui/echo")]
    async fn echo(&self, call: MethodCall) -> Value {
        json!({
            "method": call.method,
            "params": call.params,
            "session": call.session_id().map(|id| id.to_string()),
        })
    }

    /// No parameters, no result.
    #[method(name = "ui/ping")]
    async fn ping(&self) {}

    /// A tool next to the methods: one service, both families.
    #[tool]
    async fn hello(&self) -> &'static str {
        "hello"
    }
}

#[controller]
struct SecuredMethods;

#[routes]
impl SecuredMethods {
    #[method(name = "acme/whoami")]
    async fn whoami(&self, #[inject(identity)] principal: McpPrincipal) -> Value {
        json!({ "sub": principal.user.sub })
    }

    #[method(name = "acme/admin")]
    #[roles("admin")]
    async fn admin(&self, #[inject(identity)] principal: McpPrincipal) -> Value {
        json!({ "admin": principal.user.sub })
    }
}

/// Without `mcp.auth`: the identity a `JwtClaimsValidator` bean validates.
#[controller]
struct JwtMethods;

#[routes]
impl JwtMethods {
    #[method(name = "jwt/whoami")]
    async fn whoami(&self, #[inject(identity)] user: AuthenticatedUser) -> Value {
        json!({ "sub": user.sub })
    }

    #[method(name = "jwt/admin")]
    #[roles("admin")]
    async fn admin(&self, #[inject(identity)] user: AuthenticatedUser) -> Value {
        json!({ "admin": user.sub })
    }
}

/// Scopes need `mcp.auth`: on its own service.
#[controller]
struct ScopedMethods;

#[routes]
impl ScopedMethods {
    #[method(name = "acme/read", scopes = "mcp:read")]
    async fn read(&self) -> Value {
        json!({ "data": true })
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────

fn extensions() -> McpServer {
    McpServer::new()
        .with_extension(SKILLS, json!({}))
        .with_extension(APPS, json!({ "mimeTypes": ["text/html;profile=mcp-app"] }))
}

async fn app_with(server: McpServer) -> Routes {
    AppBuilder::new()
        .plugin(server)
        .build_state()
        .await
        .register_mcp_service::<SkillMethods>()
        .register_mcp_service::<AppMethods>()
        .build()
}

async fn request(router: &Routes, session: Option<&str>, method: &str, params: Value) -> Value {
    let mut body = json!({ "jsonrpc": "2.0", "id": 7, "method": method });
    if !params.is_null() {
        body["params"] = params;
    }
    let response = support::post(router, "/mcp", session, &body).await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.raw_body);
    response.message().clone()
}

// ── Serving ────────────────────────────────────────────────────────────────

#[r2e_core::test]
async fn a_method_is_served_over_stateful_http() {
    let router = app_with(extensions()).await;
    let session = support::initialize(&router, "/mcp").await;

    let rendered = request(
        &router,
        Some(&session),
        "ui/render",
        json!({ "template": "card", "width": 320 }),
    )
    .await;
    assert_eq!(
        rendered["result"],
        json!({ "html": "<card/>", "width": 320 }),
        "{rendered}"
    );

    // The raw call sees the method, its params and the session.
    let echoed = request(&router, Some(&session), "ui/echo", json!({ "k": [1, 2] })).await;
    assert_eq!(echoed["result"]["method"], "ui/echo", "{echoed}");
    assert_eq!(echoed["result"]["params"], json!({ "k": [1, 2] }));
    assert_eq!(echoed["result"]["session"], session.as_str());

    // No params, unit result: an empty object.
    let pong = request(&router, Some(&session), "ui/ping", Value::Null).await;
    assert_eq!(pong["result"], json!({}), "{pong}");

    // The tool of the same service is untouched, and a method is no tool.
    let tools = support::tools_list(&router, "/mcp", &session).await;
    assert_eq!(support::tool_names(&tools), ["hello"]);
}

#[r2e_core::test]
async fn a_method_is_served_statelessly() {
    let router = app_with(extensions().stateless(true).json_response(true)).await;

    let rendered = request(&router, None, "ui/render", json!({ "template": "row" })).await;
    assert_eq!(
        rendered["result"],
        json!({ "html": "<row/>", "width": 0 }),
        "{rendered}"
    );

    let listed = request(&router, None, "skills/list", Value::Null).await;
    assert_eq!(listed["result"]["skills"][0]["uri"], SKILL_URI, "{listed}");
}

#[r2e_core::test]
async fn an_unknown_method_is_method_not_found() {
    let router = app_with(extensions()).await;
    let session = support::initialize(&router, "/mcp").await;

    let unknown = request(&router, Some(&session), "ui/nope", json!({})).await;
    assert_eq!(unknown["error"]["code"], -32601, "{unknown}");
    assert!(unknown.get("result").is_none());
}

#[r2e_core::test]
async fn invalid_params_are_rejected() {
    let router = app_with(extensions()).await;
    let session = support::initialize(&router, "/mcp").await;

    let wrong_type = request(
        &router,
        Some(&session),
        "ui/render",
        json!({ "template": 3 }),
    )
    .await;
    assert_eq!(wrong_type["error"]["code"], -32602, "{wrong_type}");

    // `params` absent where the method needs some.
    let missing = request(&router, Some(&session), "ui/render", Value::Null).await;
    assert_eq!(missing["error"]["code"], -32602, "{missing}");
}

// ── Typed rmcp methods ─────────────────────────────────────────────────────

#[r2e_core::test]
async fn skills_methods_reach_their_routes() {
    let router = app_with(extensions()).await;
    let session = support::initialize(&router, "/mcp").await;

    let listed = request(&router, Some(&session), "skills/list", Value::Null).await;
    let skills = listed["result"]["skills"].as_array().expect("skills array");
    assert_eq!(skills.len(), 1, "{listed}");
    assert_eq!(skills[0]["uri"], SKILL_URI);
    assert_eq!(skills[0]["frontmatter"]["name"], "review");
    assert_eq!(skills[0]["resources"], "dynamic");

    let got = request(
        &router,
        Some(&session),
        "skills/get",
        json!({ "uri": SKILL_URI }),
    )
    .await;
    assert_eq!(got["result"]["skill"]["uri"], SKILL_URI, "{got}");
    assert_eq!(
        got["result"]["skill"]["frontmatter"]["description"],
        "Review a pull request"
    );

    // The route's own error, on the shared table.
    let missing = request(
        &router,
        Some(&session),
        "skills/get",
        json!({ "uri": "skill://acme/other/SKILL.md" }),
    )
    .await;
    assert_eq!(missing["error"]["code"], -32002, "{missing}");
}

#[r2e_core::test]
async fn skills_methods_without_a_route_are_method_not_found() {
    // The extension is advertised, no service implements it.
    let router = AppBuilder::new()
        .plugin(extensions())
        .build_state()
        .await
        .register_mcp_service::<AppMethods>()
        .build();
    let session = support::initialize(&router, "/mcp").await;

    let listed = request(&router, Some(&session), "skills/list", Value::Null).await;
    assert_eq!(listed["error"]["code"], -32601, "{listed}");
}

// ── Extension capabilities ─────────────────────────────────────────────────

#[r2e_core::test]
async fn extensions_are_advertised_by_initialize() {
    let router = app_with(extensions()).await;
    let response = support::post(&router, "/mcp", None, &support::initialize_body()).await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.raw_body);

    let declared = &response.result()["capabilities"]["extensions"];
    assert_eq!(declared[SKILLS], json!({}), "{declared}");
    assert_eq!(
        declared[APPS],
        json!({ "mimeTypes": ["text/html;profile=mcp-app"] })
    );
}

#[r2e_core::test]
async fn extensions_are_advertised_by_server_discover() {
    const VERSION: &str = "2026-07-28";
    let router = app_with(extensions()).await;
    let response = support::post_with_headers(
        &router,
        "/mcp",
        None,
        &[
            ("mcp-protocol-version", VERSION),
            ("mcp-method", "server/discover"),
        ],
        &json!({
            "jsonrpc": "2.0", "id": 1, "method": "server/discover",
            "params": { "_meta": {
                "io.modelcontextprotocol/protocolVersion": VERSION,
                "io.modelcontextprotocol/clientCapabilities": {}
            } }
        }),
    )
    .await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.raw_body);

    let declared = &response.result()["capabilities"]["extensions"];
    assert_eq!(declared[SKILLS], json!({}), "{declared}");
    assert!(declared[APPS].is_object(), "{declared}");
}

#[r2e_core::test]
async fn no_extension_no_capability() {
    let router = AppBuilder::new()
        .plugin(McpServer::new())
        .build_state()
        .await
        .register_mcp_service::<AppMethods>()
        .build();
    let response = support::post(&router, "/mcp", None, &support::initialize_body()).await;
    assert!(
        response.result()["capabilities"].get("extensions").is_none(),
        "{}",
        response.raw_body
    );
}

#[r2e_core::test]
async fn redeclaring_an_extension_replaces_its_settings() {
    let router = app_with(extensions().with_extension(APPS, json!({ "v": 2 }))).await;
    let response = support::post(&router, "/mcp", None, &support::initialize_body()).await;
    assert_eq!(
        response.result()["capabilities"]["extensions"][APPS],
        json!({ "v": 2 })
    );
}

// ── Guards, roles, scopes, identity (HTTP) ─────────────────────────────────

async fn secured_router(jwt: &TestJwt) -> Routes {
    AppBuilder::new()
        .provide(Arc::new(jwt.claims_validator()))
        .plugin(McpServer::new())
        .build_state()
        .await
        .register_mcp_service::<JwtMethods>()
        .build()
}

async fn request_as(router: &Routes, session: &str, bearer: &str, method: &str) -> Value {
    let response = support::post_with_headers(
        router,
        "/mcp",
        Some(session),
        &[("authorization", bearer)],
        &json!({ "jsonrpc": "2.0", "id": 9, "method": method }),
    )
    .await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.raw_body);
    response.message().clone()
}

#[r2e_core::test]
async fn a_method_is_guarded_like_a_tool() {
    let jwt = TestJwt::new();
    let router = secured_router(&jwt).await;
    let session = support::initialize(&router, "/mcp").await;
    let alice = format!("Bearer {}", jwt.token_builder("alice").roles(&["user"]).build());
    let root = format!("Bearer {}", jwt.token_builder("root").roles(&["admin"]).build());

    let me = request_as(&router, &session, &alice, "jwt/whoami").await;
    assert_eq!(me["result"], json!({ "sub": "alice" }), "{me}");

    // `#[roles]`: refused for alice, served for an admin.
    let refused = request_as(&router, &session, &alice, "jwt/admin").await;
    assert!(refused.get("result").is_none(), "{refused}");
    assert_eq!(refused["error"]["code"], -32600, "{refused}");
    let served = request_as(&router, &session, &root, "jwt/admin").await;
    assert_eq!(served["result"], json!({ "admin": "root" }), "{served}");

    // A required identity and no credentials: refused, not served.
    let anonymous = request(&router, Some(&session), "jwt/whoami", Value::Null).await;
    assert!(anonymous.get("result").is_none(), "{anonymous}");
    assert_eq!(anonymous["error"]["code"], -32600, "{anonymous}");
}

// ── Boot checks ────────────────────────────────────────────────────────────

mod boot {
    use super::*;

    #[controller]
    struct ShadowsCore;

    #[routes]
    impl ShadowsCore {
        #[method(name = "tools/list")]
        async fn list(&self) -> Value {
            json!({})
        }
    }

    #[controller]
    struct Duplicate;

    #[routes]
    impl Duplicate {
        #[method(name = "ui/render")]
        async fn render(&self) -> Value {
            json!({})
        }
    }

    #[r2e_core::test]
    #[should_panic(expected = "cannot be served")]
    async fn a_core_method_name_aborts_startup() {
        let _ = AppBuilder::new()
            .plugin(McpServer::new())
            .build_state()
            .await
            .register_mcp_service::<ShadowsCore>()
            .build();
    }

    #[r2e_core::test]
    #[should_panic(expected = "cannot be served")]
    async fn a_gated_method_without_its_extension_aborts_startup() {
        let _ = AppBuilder::new()
            .plugin(McpServer::new())
            .build_state()
            .await
            .register_mcp_service::<SkillMethods>()
            .build();
    }

    #[r2e_core::test]
    #[should_panic(expected = "duplicate MCP method name `ui/render`")]
    async fn a_duplicate_method_name_aborts_startup() {
        let _ = AppBuilder::new()
            .plugin(McpServer::new())
            .build_state()
            .await
            .register_mcp_service::<AppMethods>()
            .register_mcp_service::<Duplicate>()
            .build();
    }

    #[r2e_core::test]
    #[should_panic(expected = "declares OAuth scopes but `mcp.auth` is disabled")]
    async fn a_scoped_method_without_mcp_auth_aborts_startup() {
        let _ = AppBuilder::new()
            .plugin(McpServer::new())
            .build_state()
            .await
            .register_mcp_service::<ScopedMethods>()
            .build();
    }

    #[r2e_core::test]
    #[should_panic(expected = "extension")]
    async fn non_object_extension_settings_abort_startup() {
        let _ = AppBuilder::new()
            .plugin(McpServer::new().with_extension(APPS, json!(true)))
            .build_state()
            .await
            .register_mcp_service::<AppMethods>()
            .build();
    }
}

// ── Loopback / test client ─────────────────────────────────────────────────

#[cfg(feature = "testing")]
mod client {
    use super::*;
    use r2e_mcp::testing::{pin_mcp_validator, McpTestError, McpTestExt};
    use r2e_test::TestApp;

    const RESOURCE: &str = "http://localhost:3000/mcp";

    async fn plain_app() -> TestApp {
        let builder = AppBuilder::new()
            .plugin(extensions())
            .build_state()
            .await
            .register_mcp_service::<SkillMethods>()
            .register_mcp_service::<AppMethods>();
        TestApp::from_builder(builder)
    }

    async fn secured_app() -> TestApp {
        let jwt = TestJwt::for_resource(RESOURCE);
        let builder = pin_mcp_validator(AppBuilder::new(), &jwt, RESOURCE)
            .load_config::<()>()
            .plugin(McpServer::new())
            .build_state()
            .await
            .register_mcp_service::<SecuredMethods>()
            .register_mcp_service::<ScopedMethods>();
        TestApp::from_builder(builder).with_jwt(jwt)
    }

    #[r2e_core::test]
    async fn the_test_client_calls_an_extension_method() {
        let app = plain_app().await;
        let mcp = app.mcp().await;

        let rendered: Rendered = mcp
            .call_method("ui/render", json!({ "template": "card", "width": 12 }))
            .await
            .unwrap();
        assert_eq!(
            rendered,
            Rendered {
                html: "<card/>".into(),
                width: 12
            }
        );

        let echoed = mcp.call_method_raw("ui/echo", json!({ "a": 1 })).await.unwrap();
        assert_eq!(echoed["params"], json!({ "a": 1 }), "{echoed}");
        assert_eq!(echoed["session"], mcp.session_id());

        let pong = mcp.call_method_raw("ui/ping", ()).await.unwrap();
        assert_eq!(pong, json!({}));
    }

    #[r2e_core::test]
    async fn the_test_client_reaches_the_skills_methods() {
        let app = plain_app().await;
        let mcp = app.mcp().await;

        let listed = mcp.call_method_raw("skills/list", ()).await.unwrap();
        assert_eq!(listed["skills"][0]["uri"], SKILL_URI, "{listed}");

        let got = mcp
            .call_method_raw("skills/get", json!({ "uri": SKILL_URI }))
            .await
            .unwrap();
        assert_eq!(got["skill"]["frontmatter"]["name"], "review", "{got}");
    }

    #[r2e_core::test]
    async fn errors_surface_as_rpc_errors() {
        let app = plain_app().await;
        let mcp = app.mcp().await;

        let err = mcp.call_method_raw("ui/nope", ()).await.unwrap_err();
        assert!(
            matches!(err, McpTestError::Rpc { code: -32601, .. }),
            "{err}"
        );

        let err = mcp
            .call_method_raw("ui/render", json!({ "template": 1 }))
            .await
            .unwrap_err();
        assert!(
            matches!(err, McpTestError::Rpc { code: -32602, .. }),
            "{err}"
        );

        // A result that does not decode into the requested type.
        let err = mcp
            .call_method::<Rendered>("ui/ping", ())
            .await
            .unwrap_err();
        assert!(matches!(err, McpTestError::DecodeMethod { .. }), "{err}");
    }

    #[r2e_core::test]
    async fn scopes_roles_and_identity_apply_under_mcp_auth() {
        let app = secured_app().await;

        let alice = app
            .mcp()
            .as_user("alice", &["admin"])
            .with_scopes(&["mcp:read"])
            .await;
        assert_eq!(
            alice.call_method_raw("acme/whoami", ()).await.unwrap(),
            json!({ "sub": "alice" })
        );
        assert_eq!(
            alice.call_method_raw("acme/read", ()).await.unwrap(),
            json!({ "data": true })
        );
        assert_eq!(
            alice.call_method_raw("acme/admin", ()).await.unwrap(),
            json!({ "admin": "alice" })
        );

        // No scope, no role: both refused; the unguarded one is served.
        let bob = app.mcp().as_user("bob", &[]).await;
        assert_eq!(
            bob.call_method_raw("acme/whoami", ()).await.unwrap(),
            json!({ "sub": "bob" })
        );
        let err = bob.call_method_raw("acme/read", ()).await.unwrap_err();
        assert!(matches!(err, McpTestError::Rpc { .. }), "{err}");
        let err = bob.call_method_raw("acme/admin", ()).await.unwrap_err();
        assert!(matches!(err, McpTestError::Rpc { .. }), "{err}");
    }
}
