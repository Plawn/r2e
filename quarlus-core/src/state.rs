/// Trait alias for types that can serve as Quarlus application state.
///
/// The user's state type is used directly as the Axum router state.
/// It must be `Clone + Send + Sync + 'static`.
pub trait QuarlusState: Clone + Send + Sync + 'static {}

impl<T: Clone + Send + Sync + 'static> QuarlusState for T {}
