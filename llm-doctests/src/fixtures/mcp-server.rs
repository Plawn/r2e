//! Scaffolding for `llm/mcp-server.md`.

use r2e::prelude::*;

pub use std::sync::Arc;

/// `TestJwt` lives in the test harness crate, not the prelude.
pub use r2e_test::TestJwt;

/// The bean the MCP service injects.
#[derive(Clone)]
pub struct CalcService;

#[bean]
impl CalcService {
    pub fn new() -> Self {
        Self
    }

    pub fn add(&self, a: f64, b: f64) -> f64 {
        a + b
    }

    pub fn divide(&self, a: f64, b: f64) -> Option<f64> {
        (b != 0.0).then(|| a / b)
    }
}

/// The bean behind the `#[resource]` snippet.
#[derive(Clone)]
pub struct CallLog;

#[bean]
impl CallLog {
    pub fn new() -> Self {
        Self
    }

    pub fn entries(&self) -> Vec<String> {
        Vec::new()
    }
}

/// The guard the `clear` tool applies — a dep-free `SelfBuilt` guard, exactly
/// like `llm/guards.md`'s `BlockUser`.
pub struct ApiKeyGuard;

impl SelfBuilt for ApiKeyGuard {}

impl<I: Identity> Guard<I> for ApiKeyGuard {
    async fn check(&self, ctx: &GuardContext<'_, I>) -> Result<(), r2e::http::Response> {
        if ctx.headers.contains_key("x-api-key") {
            Ok(())
        } else {
            Err(GuardError::unauthorized("missing x-api-key").into())
        }
    }
}

/// Tool arguments/result shared by the snippets.
#[derive(serde::Deserialize, schemars::JsonSchema, ObjectParams)]
pub struct AddIn {
    pub a: f64,
    pub b: f64,
}

#[derive(serde::Serialize, schemars::JsonSchema)]
pub struct AddOut {
    pub value: f64,
}

/// The service `register_mcp_service::<MathTools>()` names (the doc blocks
/// declare their own richer copies).
#[controller]
pub struct MathTools {
    #[inject]
    calc: CalcService,
}

#[mcp_routes]
impl MathTools {
    /// Add two numbers.
    #[tool(read_only, idempotent)]
    async fn add(&self, Params(p): Params<AddIn>) -> Json<AddOut> {
        Json(AddOut {
            value: self.calc.add(p.a, p.b),
        })
    }
}
