pub trait Controller<T: Clone + Send + Sync + 'static> {
    fn routes() -> axum::Router<T>;

    /// Return metadata about all routes registered by this controller.
    /// Used for OpenAPI spec generation. Default returns an empty list.
    fn route_metadata() -> Vec<crate::openapi::RouteInfo> {
        Vec::new()
    }

    /// Register event consumers for this controller.
    ///
    /// Called during `serve()` with the application state, before startup hooks.
    /// The default implementation does nothing.
    fn register_consumers(
        _state: T,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
        Box::pin(async {})
    }
}
