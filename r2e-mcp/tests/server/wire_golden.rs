//! Wire-format goldens for the four `*/list` families.
//!
//! The list payloads are built once at boot and cloned per request
//! (`handler::Family`), and the metadata they are built from is stored in
//! `Cow<'static, str>` where rmcp allows it (task #994). Both are
//! representation changes that must be invisible on the wire — this target
//! pins the exact JSON so a future allocation optimisation cannot quietly
//! drop a field, reorder a schema or change a description.
//!
//! Comparison is on the PARSED `serde_json::Value` (object key order is
//! irrelevant, array order is not — list order is registration order and is
//! part of the contract).
//!
//! Re-baseline deliberately, after eyeballing the diff:
//!
//! ```bash
//! R2E_UPDATE_GOLDEN=1 cargo test -p r2e-mcp --test server wire_golden::
//! ```
//!
//! # Provenance of the committed goldens
//!
//! These files landed in the same commit as the change they guard, so on their
//! own they would only prove the new code agrees with itself. They were
//! checked against the pre-change tree instead: this file compiles unchanged
//! against master (it only uses `#[mcp_routes]`'s public surface), so master
//! can emit its own goldens.
//!
//! ```bash
//! git checkout -b tmp/golden-provenance c8da199   # master, pre-#994/#993
//! git show <pr-branch>:r2e-mcp/tests/server/wire_golden.rs > $PWD/r2e-mcp/tests/server/wire_golden.rs
//! echo 'mod wire_golden;' >> r2e-mcp/tests/server/main.rs
//! R2E_UPDATE_GOLDEN=1 cargo test -p r2e-mcp --test server wire_golden::
//! # then diff the emitted golden/ against the one committed on the PR branch
//! ```
//!
//! Run on **c8da199** (`Merge pull request #55`, this branch's merge base) all
//! four files came out byte-identical to the ones committed here.

use r2e_core::http::Router;
use r2e_core::prelude::*;
use r2e_core::AppBuilder;
use r2e_mcp::{AppBuilderMcpExt, McpError, McpServer, Params, ResourceCall};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::support;

// ── A service covering every wire field the list builders can emit ─────────

#[derive(Deserialize, JsonSchema, ObjectParams)]
pub struct GoldenInput {
    /// A documented required field.
    pub name: String,
    /// An optional count.
    pub count: Option<u32>,
}

#[derive(Serialize, JsonSchema)]
pub struct GoldenOutput {
    /// The echoed name.
    pub value: String,
}

#[controller]
struct GoldenService;

#[mcp_routes]
impl GoldenService {
    /// Echo a name.
    ///
    /// The body of the doc comment is part of the advertised description.
    #[tool(read_only, idempotent, title = "Echo")]
    async fn echo(&self, Params(p): Params<GoldenInput>) -> Json<GoldenOutput> {
        Json(GoldenOutput {
            value: format!("{}:{}", p.name, p.count.unwrap_or(0)),
        })
    }

    /// Fail on demand.
    #[tool(name = "boom", destructive, open_world)]
    async fn explode(&self) -> Result<String, McpError> {
        Err(McpError::tool("boom"))
    }

    /// A tool with no annotations and no arguments.
    #[tool]
    async fn plain(&self) -> String {
        "plain".to_string()
    }

    /// A fixed resource.
    #[resource(
        uri = "r2e://golden/info",
        name = "info",
        title = "Golden info",
        mime_type = "text/plain"
    )]
    async fn info(&self) -> &'static str {
        "info"
    }

    /// A templated resource.
    #[resource(uri = "r2e://golden/users/{id}", mime_type = "application/json")]
    async fn user(&self, call: ResourceCall) -> String {
        format!("user:{}", call.variables["id"])
    }

    /// Explain the echo tool.
    ///
    /// Walks the agent through calling `echo`.
    #[prompt(name = "explain_echo", title = "Explain echo")]
    async fn explain(&self, Params(p): Params<GoldenInput>) -> String {
        format!("Call `echo` with {}.", p.name)
    }

    /// A prompt without arguments.
    #[prompt]
    async fn usage(&self) -> String {
        "Use `echo`.".to_string()
    }
}

async fn golden_app() -> Router {
    AppBuilder::new()
        .plugin(McpServer::new())
        .build_state()
        .await
        .register_mcp_service::<GoldenService>()
        .build()
}

// ── Golden plumbing ────────────────────────────────────────────────────────

fn golden_path(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/server/golden")
        .join(format!("{name}.json"))
}

#[track_caller]
fn assert_golden(name: &str, actual: &Value) {
    let path = golden_path(name);
    if std::env::var_os("R2E_UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).expect("create golden dir");
        let mut rendered = serde_json::to_string_pretty(actual).expect("render golden");
        rendered.push('\n');
        std::fs::write(&path, rendered).expect("write golden");
        return;
    }
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "missing golden {}: {e}\nre-create it with R2E_UPDATE_GOLDEN=1",
            path.display()
        )
    });
    let expected: Value = serde_json::from_str(&raw).expect("golden is not JSON");
    assert_eq!(
        actual,
        &expected,
        "{name}: the `*/list` wire payload changed.\nIf the change is intended, \
         re-baseline with R2E_UPDATE_GOLDEN=1 and review the diff.\ngot: {}",
        serde_json::to_string_pretty(actual).unwrap()
    );
}

async fn list(router: &Router, session: &str, id: i64, method: &str) -> Value {
    let response = support::post(
        router,
        "/mcp",
        Some(session),
        &json!({ "jsonrpc": "2.0", "id": id, "method": method }),
    )
    .await;
    response.result().clone()
}

#[r2e_core::test]
async fn list_payloads_match_the_wire_goldens() {
    let router = golden_app().await;
    let session = support::initialize(&router, "/mcp").await;

    assert_golden("tools_list", &list(&router, &session, 2, "tools/list").await);
    assert_golden(
        "resources_list",
        &list(&router, &session, 3, "resources/list").await,
    );
    assert_golden(
        "resource_templates_list",
        &list(&router, &session, 4, "resources/templates/list").await,
    );
    assert_golden(
        "prompts_list",
        &list(&router, &session, 5, "prompts/list").await,
    );
}

/// A second `*/list` on the same session must be byte-identical to the
/// first: the payload is prebuilt and cloned, never rebuilt per request.
#[r2e_core::test]
async fn repeated_lists_are_identical() {
    let router = golden_app().await;
    let session = support::initialize(&router, "/mcp").await;

    for method in [
        "tools/list",
        "resources/list",
        "resources/templates/list",
        "prompts/list",
    ] {
        let first = list(&router, &session, 6, method).await;
        let second = list(&router, &session, 7, method).await;
        assert_eq!(first, second, "{method} is not stable across requests");
    }
}
