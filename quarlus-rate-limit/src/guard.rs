use axum::extract::FromRef;
use axum::response::IntoResponse;
use quarlus_core::guards::{Guard, GuardContext};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateLimitKeyKind {
    Global,
    User,
    Ip,
}

pub struct RateLimitGuard {
    pub max: u64,
    pub window_secs: u64,
    pub key: RateLimitKeyKind,
}

impl<S> Guard<S> for RateLimitGuard
where
    crate::RateLimitRegistry: FromRef<S>,
{
    fn check(
        &self,
        state: &S,
        ctx: &GuardContext,
    ) -> Result<(), axum::response::Response> {
        let registry = <crate::RateLimitRegistry as FromRef<S>>::from_ref(state);
        let method = ctx.method_name;
        let key = match self.key {
            RateLimitKeyKind::Global => format!("{}:global", method),
            RateLimitKeyKind::User => {
                let sub = ctx.identity_sub.unwrap_or("anonymous");
                format!("{}:user:{}", method, sub)
            }
            RateLimitKeyKind::Ip => {
                let ip = ctx
                    .headers
                    .get("x-forwarded-for")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.split(',').next())
                    .map(|s| s.trim())
                    .unwrap_or("unknown");
                format!("{}:ip:{}", method, ip)
            }
        };
        if registry.try_acquire(&key, self.max, self.window_secs) {
            Ok(())
        } else {
            Err((
                axum::http::StatusCode::TOO_MANY_REQUESTS,
                axum::Json(serde_json::json!({ "error": "Rate limit exceeded" })),
            )
                .into_response())
        }
    }
}
