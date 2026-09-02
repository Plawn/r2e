//! Scaffolding for `llm/events.md`.

use r2e::prelude::*;
use r2e::BeanContext;

pub use std::sync::Arc;
pub use std::time::Duration;

pub use serde::{Deserialize, Serialize};

/// The fan-out event every snippet emits.
#[derive(Clone, Serialize, Deserialize)]
pub struct UserCreatedEvent {
    pub user_id: i64,
}

/// Request-reply pair.
#[derive(Clone, Serialize, Deserialize)]
pub struct GreetRequest {
    pub name: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct GreetReply {
    pub message: String,
}

/// The event the interceptor snippet consumes.
#[derive(Clone, Serialize, Deserialize)]
pub struct Ping;

/// A collaborator a consumer bean injects.
#[derive(Clone)]
pub struct Mailer;

#[bean]
impl Mailer {
    pub fn new() -> Self {
        Self
    }

    pub async fn send(&self, _to: &str) {}
}

/// A user-defined interceptor spec (`#[intercept(Audit::spec("…"))]`).
pub struct Audit {
    label: &'static str,
}

impl Audit {
    pub fn spec(label: &'static str) -> Self {
        Self { label }
    }
}

pub struct AuditInterceptor {
    pub label: &'static str,
}

impl<R> Interceptor<R> for AuditInterceptor {
    fn around<F, Fut>(&self, _ctx: InterceptorContext, next: F) -> impl Future<Output = R> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = R> + Send,
    {
        let label = self.label;
        async move {
            tracing::debug!(audit = label, "enter");
            next().await
        }
    }
}

impl DecoratorSpec for Audit {
    type Product = AuditInterceptor;
    type Deps = r2e::type_list::TNil;

    fn build(self, _ctx: &BeanContext) -> AuditInterceptor {
        AuditInterceptor { label: self.label }
    }
}
