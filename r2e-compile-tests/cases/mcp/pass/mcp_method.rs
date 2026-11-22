//! `#[method]` — an extension JSON-RPC method next to a tool, with every
//! supported parameter and return shape.

use r2e::prelude::*;
use r2e::r2e_core::rt::CancelToken;
use r2e::r2e_mcp::{McpError, McpSession, Progress};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize)]
pub struct GetSkill {
    pub uri: String,
}

#[controller]
pub struct SkillMethods {}

#[routes]
impl SkillMethods {
    #[method(name = "skills/list")]
    async fn list(&self) -> Value {
        json!({ "skills": [] })
    }

    #[method(name = "skills/get", scopes = "skills:read")]
    async fn get(&self, Params(p): Params<GetSkill>) -> Result<Value, McpError> {
        Err(McpError::NotFound(p.uri))
    }

    #[method(name = "acme/raw")]
    async fn raw(
        &self,
        call: MethodCall,
        _cancel: CancelToken,
        _session: McpSession,
        _progress: Progress,
    ) -> Json<String> {
        Json(call.method.to_string())
    }

    #[method(name = "acme/ping")]
    async fn ping(&self) {}

    #[tool]
    async fn hello(&self) -> &'static str {
        "hello"
    }
}

fn main() {}
