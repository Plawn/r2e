use quarlus_core::guards::{Guard, GuardContext, Identity};
use quarlus_core::http::extract::FromRef;
use quarlus_core::http::response::IntoResponse;

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

impl<S, I: Identity> Guard<S, I> for RateLimitGuard
where
    crate::RateLimitRegistry: FromRef<S>,
{
    fn check(
        &self,
        state: &S,
        ctx: &GuardContext<'_, I>,
    ) -> Result<(), quarlus_core::http::Response> {
        let registry = <crate::RateLimitRegistry as FromRef<S>>::from_ref(state);
        let method = ctx.method_name;
        let key = match self.key {
            RateLimitKeyKind::Global => format!("{}:global", method),
            RateLimitKeyKind::User => {
                let sub = ctx
                    .identity
                    .map(|i| i.sub())
                    .unwrap_or("anonymous");
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
                quarlus_core::http::StatusCode::TOO_MANY_REQUESTS,
                quarlus_core::http::Json(serde_json::json!({ "error": "Rate limit exceeded" })),
            )
                .into_response())
        }
    }
}
