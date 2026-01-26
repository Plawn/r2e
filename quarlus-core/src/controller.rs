pub trait Controller<T: Clone + Send + Sync + 'static> {
    fn routes() -> axum::Router<T>;
}
