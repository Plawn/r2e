//! The injectable app shutdown signal (`rt::ShutdownToken`).
//!
//! Three properties are pinned here:
//!
//! 1. it is a **normal bean** — on the provision list from `AppBuilder::new()`,
//!    in the state HList, resolvable as a dependency of another bean;
//! 2. it is on the **app's shutdown lineage** — the graceful drain and the
//!    uncontrolled-exit drop guard both cancel it;
//! 3. it is **overridable**, which is how a test drives it by hand (the bean
//!    has no `cancel()` of its own — see `r2e_rt::ShutdownToken`).

use std::any::{type_name, TypeId};
use std::time::Duration;

use r2e_core::beans::{Bean, BeanContext, BeanRegistry, Registrable};
use r2e_core::rt::{CancelToken, ShutdownToken};
use r2e_core::type_list::{BeanAccess, TCons, TNil};
use r2e_core::AppBuilder;

/// A bean that declares the shutdown token as a compile-time dependency —
/// exactly what `#[inject] shutdown: ShutdownToken` expands to.
#[derive(Clone)]
struct Worker {
    shutdown: ShutdownToken,
}

impl Bean for Worker {
    type Error = ::std::convert::Infallible;
    type Deps = TCons<ShutdownToken, TNil>;
    fn dependencies() -> Vec<(TypeId, &'static str)> {
        vec![(
            TypeId::of::<ShutdownToken>(),
            type_name::<ShutdownToken>(),
        )]
    }
    fn build(ctx: &BeanContext) -> ::std::result::Result<Self, Self::Error> {
        ::std::result::Result::Ok(Worker {
            shutdown: ctx.get::<ShutdownToken>(),
        })
    }
}

impl Registrable for Worker {
    type Provided = Self;
    // The shutdown token has to be on the provision list for this to compile:
    // `register::<Worker>()` folds `Deps` into the builder's requirement list
    // and `build_state()` checks it with `AllSatisfied`.
    type Deps = TCons<ShutdownToken, TNil>;
    fn register_into(registry: &mut BeanRegistry) {
        registry.register::<Self>();
    }
}

#[r2e_core::test]
async fn shutdown_token_is_in_every_state_hlist() {
    let app = AppBuilder::new().build_state().await;
    let token: ShutdownToken = app.state().get::<ShutdownToken>();
    assert!(
        !token.is_cancelled(),
        "a freshly built app must not look like it is shutting down"
    );
    // Same instance through the retained bean context.
    assert!(app.bean_context().try_get::<ShutdownToken>().is_some());
}

#[r2e_core::test]
async fn shutdown_token_resolves_as_a_bean_dependency() {
    let app = AppBuilder::new().register::<Worker>().build_state().await;
    let worker = app.bean_context().try_get::<Worker>().unwrap();
    assert!(!worker.shutdown.is_cancelled());
}

#[r2e_core::test]
async fn override_bean_replaces_the_builtin_token() {
    // `AppBuilder::new()` provides the token before user code runs, so the
    // override has to win over an already-provided value — `pin_provide`
    // semantics, not "first one wins".
    let mine = CancelToken::new();
    let app = AppBuilder::new()
        .override_bean(ShutdownToken::from_token(mine.clone()))
        .register::<Worker>()
        .build_state()
        .await;
    let worker = app.bean_context().try_get::<Worker>().unwrap();
    assert!(!worker.shutdown.is_cancelled());
    mine.cancel();
    assert!(
        worker.shutdown.is_cancelled(),
        "the bean must be the overridden token, not the builder's own"
    );
}

#[r2e_core::test]
async fn child_token_does_not_cancel_the_app() {
    let app = AppBuilder::new().build_state().await;
    let token: ShutdownToken = app.state().get::<ShutdownToken>();
    let scope = token.child_token();
    scope.cancel();
    assert!(scope.is_cancelled());
    assert!(
        !token.is_cancelled(),
        "cancelling a user scope must not take the application down"
    );
}

#[tokio::test]
async fn graceful_drain_cancels_the_injected_token() {
    let app = AppBuilder::new().build_state().await;
    let token: ShutdownToken = app.state().get::<ShutdownToken>();
    let prepared = app.prepare("127.0.0.1:0");
    let stop = prepared.stop_handle();
    assert!(!token.is_cancelled());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server = r2e_core::rt::spawn(async move {
        prepared
            .run_with_listener(listener)
            .await
            .map_err(|e| e.to_string())
    });

    stop.stop();
    let result = tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("server did not stop within 5s")
        .expect("server task panicked");
    assert!(result.is_ok(), "run() returned an error: {result:?}");

    assert!(
        token.is_cancelled(),
        "the graceful drain must cancel the injectable shutdown token"
    );
}

#[tokio::test]
async fn dropping_the_run_future_cancels_the_injected_token() {
    // The uncontrolled-exit path: an `r2e dev` hot patch drops the `run()`
    // future outright. The drop guard armed by `run()` must still fire, or a
    // carried-over SSE stream would outlive its cycle.
    let app = AppBuilder::new().build_state().await;
    let token: ShutdownToken = app.state().get::<ShutdownToken>();
    let prepared = app.prepare("127.0.0.1:0");

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut fut = Box::pin(prepared.run_with_listener(listener));
    // Poll once so the run loop actually starts (and arms its guard).
    let started = tokio::time::timeout(Duration::from_millis(50), &mut fut).await;
    assert!(started.is_err(), "run() should still be serving");
    drop(fut);

    assert!(
        token.is_cancelled(),
        "dropping run() must cancel the injectable shutdown token"
    );
}
