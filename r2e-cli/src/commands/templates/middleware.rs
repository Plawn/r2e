pub fn interceptor(name: &str) -> String {
    format!(
        r#"use r2e::prelude::*;
use std::future::Future;

/// Custom interceptor: {name}
///
/// Self-contained (no bean deps) — the `SelfBuilt` opt-in makes it usable in
/// `#[intercept({name})]`. To read beans instead, replace `SelfBuilt` with
/// `#[derive(DecoratorBean)]`, mark the bean fields `#[inject]`, and apply
/// with `#[intercept({name}::spec(...))]` (see the r2e book).
pub struct {name};

impl SelfBuilt for {name} {{}}

impl<R: Send> Interceptor<R> for {name} {{
    fn around<F, Fut>(&self, ctx: InterceptorContext, next: F) -> impl Future<Output = R> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = R> + Send,
    {{
        let method_name = ctx.method_name;
        async move {{
            tracing::info!(method = method_name, "{name}: before");
            let result = next().await;
            tracing::info!(method = method_name, "{name}: after");
            result
        }}
    }}
}}
"#
    )
}
