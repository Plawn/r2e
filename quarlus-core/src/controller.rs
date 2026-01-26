pub trait Controller<T: Clone + Send + Sync + 'static> {
    fn routes() -> axum::Router<T>;

    /// Return metadata about all routes registered by this controller.
    /// Used for OpenAPI spec generation. Default returns an empty list.
    fn route_metadata() -> Vec<crate::openapi::RouteInfo> {
        Vec::new()
    }
}
