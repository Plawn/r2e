//! Phase 4: SSE and WebSocket route methods with struct-level identity run on
//! the façade. The façade owns its identity for the whole stream/socket future,
//! so `self.user` is available throughout.

use futures_core::Stream;
use r2e::prelude::*;
use r2e::r2e_security::AuthenticatedUser;
use r2e::r2e_security::JwtClaimsValidator;
use r2e::ws::WsStream;
use r2e::Identity;
use std::convert::Infallible;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub claims_validator: Arc<JwtClaimsValidator>,
}

impl FromRef<AppState> for Arc<JwtClaimsValidator> {
    fn from_ref(state: &AppState) -> Self {
        state.claims_validator.clone()
    }
}

#[controller(path = "/stream", state = AppState)]
pub struct StreamController {
    #[inject(identity)]
    user: AuthenticatedUser,
}

#[routes]
impl StreamController {
    #[sse("/events")]
    async fn events(&self) -> impl Stream<Item = Result<SseEvent, Infallible>> {
        let sub = self.user.sub().to_string();
        futures_util::stream::once(async move { Ok(SseEvent::default().data(sub)) })
    }

    #[ws("/socket")]
    async fn socket(&self, mut ws: WsStream) {
        let sub = self.user.sub().to_string();
        ws.send_text(&sub).await.ok();
    }
}

fn main() {}
