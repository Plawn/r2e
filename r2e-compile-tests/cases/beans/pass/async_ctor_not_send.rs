//! Async bean constructors and producers must NOT be `Send`-bound.
//!
//! `AsyncBean::build` / `Producer::produce` used to return
//! `impl Future<Output = _> + Send + '_`. Because `+ Send` on an RPITIT is
//! checked for *all* lifetimes, it rejected perfectly ordinary bodies — sqlx's
//! `Acquire`/`Executor` reborrows above all — with
//!
//!     error: lifetime bound not satisfied
//!       = note: this is a known limitation that will be removed in the future
//!               (see issue #100013)
//!
//! (older rustc rendered the same thing as "implementation of `Executor` is not
//! general enough … `Executor<'1>` would have to be implemented for the type
//! `&'0 mut PgConnection`, for any two lifetimes `'0` and `'1`").
//!
//! The graph is built and awaited in place on the boot thread, so the bound was
//! never needed. This case is the regression guard.

use r2e::prelude::*;
use sqlx::{Acquire, Sqlite, SqlitePool};

/// A generic helper over `Acquire` — the ordinary way real code shares
/// "run this against a pool or a transaction" logic. This is the shape that
/// tripped the old `+ Send` bound.
pub async fn with_conn<'a, A>(acq: A)
where
    A: Acquire<'a, Database = Sqlite> + Send,
    A::Connection: Send,
{
    let mut conn = acq.acquire().await.unwrap();
    sqlx::query("SELECT 1").execute(&mut *conn).await.unwrap();
}

#[derive(Clone)]
pub struct Marker;

#[producer]
async fn make_marker(pool: SqlitePool) -> Marker {
    let mut tx = pool.begin().await.unwrap();
    with_conn(&mut tx).await;
    tx.commit().await.unwrap();
    Marker
}

#[derive(Clone)]
pub struct Svc;

#[bean]
impl Svc {
    pub async fn new(pool: SqlitePool) -> Self {
        let mut tx = pool.begin().await.unwrap();
        with_conn(&mut tx).await;
        tx.commit().await.unwrap();
        Self
    }
}

/// The bound is gone outright, not merely widened: a constructor may hold an
/// outright `!Send` value across an await.
#[derive(Clone)]
pub struct NotSendCtor;

#[bean]
impl NotSendCtor {
    pub async fn new() -> Self {
        let local = std::rc::Rc::new(1u8);
        r2e::rt::yield_now().await;
        let _ = *local;
        Self
    }
}

fn main() {}
