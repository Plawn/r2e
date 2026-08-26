//! Controller-core `#[on_start]` startup observers.
//!
//! Controller hooks join the same globally ordered list as bean hooks: sorted
//! by declared `order`, ties in registration order (bean hooks, collected at
//! `build_state()`, before controller hooks). They run at startup — including
//! under `build_with_consumers` (the `TestApp::boot` path) — and an `Err`
//! aborts boot.

use std::sync::{Arc, Mutex};

use r2e_core::prelude::*;

type Log = Arc<Mutex<Vec<&'static str>>>;

#[controller(path = "/on-start")]
struct StartController {
    #[inject]
    log: Log,
}

#[routes]
impl StartController {
    #[get("/")]
    async fn index(&self) -> &'static str {
        "ok"
    }

    /// Runs after the `order = 0` default below (ascending order).
    #[on_start(order = 5)]
    async fn second(&self) {
        self.log.lock().unwrap().push("controller-late");
    }

    #[on_start]
    fn first(&self) {
        self.log.lock().unwrap().push("controller-default");
    }
}

/// A bean observer, to pin down bean-before-controller ordering at equal
/// `order`.
#[derive(Clone)]
struct StartBean {
    log: Log,
}

#[r2e_macros::bean]
impl StartBean {
    fn new(log: Log) -> Self {
        Self { log }
    }

    #[on_start]
    async fn observe(&self) {
        self.log.lock().unwrap().push("bean-default");
    }
}

#[r2e_core::test]
async fn controller_on_start_runs_ordered_with_bean_hooks() {
    let log: Log = Arc::new(Mutex::new(Vec::new()));

    let _router = r2e_core::AppBuilder::new()
        .provide(log.clone())
        .register::<StartBean>()
        .build_state()
        .await
        .register_controller::<StartController>()
        .build_with_consumers()
        .await;

    assert_eq!(
        *log.lock().unwrap(),
        vec!["bean-default", "controller-default", "controller-late"],
        "equal order keeps registration order: bean hooks, then controller hooks"
    );
}

#[controller(path = "/on-start-fail")]
struct FailingStartController {}

#[routes]
impl FailingStartController {
    #[get("/")]
    async fn index(&self) -> &'static str {
        "ok"
    }

    #[on_start]
    async fn boom(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Err("controller on-start boom".into())
    }
}

#[tokio::test]
async fn controller_on_start_error_aborts_boot() {
    let app = r2e_core::AppBuilder::new()
        .build_state()
        .await
        .register_controller::<FailingStartController>();

    let prepared = app.prepare("127.0.0.1:0");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let err = prepared
        .run_with_listener(listener)
        .await
        .expect_err("a controller #[on_start] Err must abort boot");
    assert!(
        err.to_string().contains("controller on-start boom"),
        "error must carry the hook's message, got: {err}"
    );
}
