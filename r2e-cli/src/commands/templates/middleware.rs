pub fn interceptor(name: &str) -> String {
    format!(
        r#"use r2e::prelude::*;
use std::future::Future;

/// Custom interceptor: {name}
pub struct {name};

impl<R: Send> Interceptor<R> for {name} {{
    fn around<F, Fut>(&self, ctx: InterceptorContext, next: F) -> impl Future<Output = R> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = R> + Send,
    {{
        async move {{
            tracing::info!(method = ctx.method_name, "{name}: before");
            let result = next().await;
            tracing::info!(method = ctx.method_name, "{name}: after");
            result
        }}
    }}
}}
"#
    )
}
