use std::future::Future;

/// Context passed to each interceptor. `Copy` so it can be captured
/// by nested `async move` closures without ownership issues.
#[derive(Clone, Copy)]
pub struct InterceptorContext {
    pub method_name: &'static str,
    pub controller_name: &'static str,
}

/// Generic interceptor trait with an `around` pattern.
///
/// Each interceptor wraps the next computation. Interceptors are composed
/// by nesting: the outermost interceptor calls `next()` which runs the
/// next interceptor, and so on.
///
/// Type parameter `R` is the return type of the wrapped computation.
/// Interceptors that don't constrain the return type (e.g. `Logged`, `Timed`)
/// are generic over all `R: Send`. Interceptors that need specific capabilities
/// (e.g. `Cache` needs `Serialize + DeserializeOwned`) constrain `R` accordingly.
pub trait Interceptor<R> {
    fn around<F, Fut>(&self, ctx: InterceptorContext, next: F) -> impl Future<Output = R> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = R> + Send;
}
