pub mod interceptors;
pub use interceptors::{LogLevel, log_at_level, Logged, Timed, Cache, CacheInvalidate};

pub mod prelude {
    //! Re-exports of the most commonly used utility interceptors.
    pub use crate::interceptors::{Cache, CacheInvalidate, Logged, Timed};
}
