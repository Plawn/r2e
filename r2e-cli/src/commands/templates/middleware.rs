pub fn interceptor(name: &str) -> String {
    format!(
        r#"use r2e::prelude::*;
use std::future::Future;

/// Custom interceptor: {name}
pub struct {name};

impl<R: Send, S: Send + Sync> Interceptor<R, S> for {name} {{
    fn around<F, Fut>(&self, ctx: InterceptorContext<'_, S>, next: F) -> impl Future<Output = R> + Send
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
