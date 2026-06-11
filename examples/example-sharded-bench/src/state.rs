use r2e::prelude::*;

/// Minimal shared state for the benchmark app.
///
/// Holds only a sqlite connection pool — the point is to measure HTTP serving,
/// not framework features. The pool is built once on the control plane and
/// shared (cloned) across all sharded workers.
#[derive(Clone, BeanState)]
pub struct Services {
    pub pool: sqlx::SqlitePool,
}
