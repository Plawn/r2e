/// Trait alias for types that can serve as R2E application state.
///
/// The user's state type is used directly as the Axum router state.
/// It must be `Clone + Send + Sync + 'static`.
pub trait R2eState: Clone + Send + Sync + 'static {}

impl<T: Clone + Send + Sync + 'static> R2eState for T {}
