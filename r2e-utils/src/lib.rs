pub mod interceptors;
pub use interceptors::{
    log_at_level, Cache, CacheInvalidate, Counted, LogLevel, Logged, MetricTimed, Timed,
};

pub mod prelude {
    //! Re-exports of the most commonly used utility interceptors.
    pub use crate::interceptors::{Cache, CacheInvalidate, Counted, Logged, MetricTimed, Timed};
}
