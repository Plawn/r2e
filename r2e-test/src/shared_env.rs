//! [`SharedEnv`]: one [`App::Env`] per test binary, built on a runtime that
//! outlives every test.
//!
//! # Why this type exists (and a plain `OnceCell` does not work)
//!
//! `App::Env` is the production concept of "resources built once" — a pool, a
//! migrated schema, a container handle. Sharing one across boots is what makes
//! a test binary stop replaying that work per test
//! ([`TestApp::boot_env`](crate::TestApp::boot_env) and friends).
//!
//! The obvious way to memoise it is wrong:
//!
//! ```ignore
//! // ✗ DO NOT DO THIS
//! static ENV: OnceCell<MyEnv> = OnceCell::const_new();
//! ENV.get_or_init(|| async { MyApp::setup().await.unwrap() }).await.clone()
//! ```
//!
//! `#[r2e::test]` builds **one runtime per test** and drops it when the test
//! returns; `#[r2e::test_suite]` builds one per suite and shuts it down after
//! the last case. A `OnceCell` initialised that way runs `setup()` on whichever
//! per-test runtime happened to win the race, so everything the environment
//! binds to that runtime — listeners, sockets, pool keep-alive tasks, timers,
//! anything `setup` spawned — is destroyed with it. The value survives in the
//! `static`; the reactor behind it does not. Later tests then reuse an inert
//! environment and hang or time out, at a distance from the test that "owned"
//! the runtime. (A `LazyLock` that builds its own runtime and `block_on`s it is
//! worse: called from inside a test runtime it panics outright.)
//!
//! [`SharedEnv`] fixes the lifetime rather than the symptom: it runs `setup`
//! **once**, on a multi-thread runtime this crate owns in a `static` and never
//! shuts down, so the environment's reactor lives as long as the test process.
//! Callers await the result from their own runtime, which is fine — a resource
//! is driven by the runtime that created it, not by the one polling it.
//!
//! ```ignore
//! use r2e_test::{SharedEnv, TestApp};
//!
//! static ENV: SharedEnv<MyApp> = SharedEnv::new();
//!
//! #[r2e::test(app = MyApp, env = ENV.get().await)]
//! async fn lists_users(app: TestApp) {
//!     app.get("/users").send().await.assert_ok();
//! }
//!
//! #[r2e::test(app = MyApp, env = ENV.get().await)]
//! async fn creates_a_user(app: TestApp) {
//!     app.post("/users").json(&user).send().await.assert_status(201);
//! }
//! ```

use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::OnceLock;

use r2e_core::rt::sync::watch;
use r2e_core::rt::{Runtime, RuntimeBuilder, RuntimeHandle};
use r2e_core::{App, BootError};

use crate::boot::error_chain;

/// The future a custom [`SharedEnv::with`] initializer returns.
///
/// Boxed because a `static` stores a plain `fn` pointer: `|| Box::pin(async {
/// … })` is the whole ceremony.
pub type SharedEnvFuture<E> = Pin<Box<dyn Future<Output = Result<E, BootError>> + Send>>;

/// A process-lifetime [`App::Env`], built once, on a runtime that outlives
/// every test.
///
/// Declare it in a `static` (the constructor is `const`) and hand
/// [`get`](Self::get) to as many boots as the binary needs:
///
/// ```ignore
/// static ENV: SharedEnv<MyApp> = SharedEnv::new();
///
/// #[r2e::test(app = MyApp, env = ENV.get().await)]
/// async fn lists_users(app: TestApp) { … }
///
/// // Explicit form:
/// let app = TestApp::boot_env::<MyApp>(ENV.get().await).await;
/// ```
///
/// See the [module docs](self) for why the environment must not be built on a
/// per-test runtime.
///
/// # Isolation is yours
///
/// One environment means **shared state** — the same pool, the same rows, the
/// same caches for every test that boots off it, with the boots still running
/// concurrently under libtest. Keep the tests independent (per-test schema,
/// prefix or tenant, unique fixtures) or serialise them with
/// `#[r2e::test(order = …)]`.
pub struct SharedEnv<A: App> {
    /// `None` = build with `A::setup()`; `Some(f)` = build with the custom
    /// initializer from [`SharedEnv::with`].
    init: Option<fn() -> SharedEnvFuture<A::Env>>,
    /// Set exactly once, by the caller that wins `get_or_init`. That caller
    /// starts the single `setup` run; every caller (winner included) then waits
    /// on the channel, so a caller whose test is cancelled mid-wait cannot
    /// cause a second `setup`.
    slot: OnceLock<watch::Sender<Slot<A::Env>>>,
    _app: PhantomData<fn() -> A>,
}

/// `None` while `setup` is still running; `Some` once it has finished, for
/// good — a failed environment stays failed for the whole process.
type Slot<E> = Option<Result<E, SharedEnvError>>;

impl<A: App + 'static> SharedEnv<A> {
    /// A shared environment built by `A::setup()`.
    ///
    /// `const`, so it goes straight into a `static`. Nothing runs until the
    /// first [`get`](Self::get)/[`try_get`](Self::try_get).
    #[must_use]
    pub const fn new() -> Self {
        Self {
            init: None,
            slot: OnceLock::new(),
            _app: PhantomData,
        }
    }

    /// A shared environment built by a **custom** initializer instead of
    /// `A::setup()` — the place for "setup, then seed once":
    ///
    /// ```ignore
    /// static ENV: SharedEnv<MyApp> = SharedEnv::with(|| {
    ///     Box::pin(async {
    ///         let env = MyApp::setup().await?;
    ///         seed_reference_data(&env).await?;
    ///         Ok(env)
    ///     })
    /// });
    /// ```
    ///
    /// The initializer runs under the same one-shot, one-runtime contract as
    /// [`new`](Self::new): once per process, on the shared runtime.
    #[must_use]
    pub const fn with(init: fn() -> SharedEnvFuture<A::Env>) -> Self {
        Self {
            init: Some(init),
            slot: OnceLock::new(),
            _app: PhantomData,
        }
    }

    /// The shared environment, building it on first use.
    ///
    /// Concurrent first callers all wait on the **one** run: `setup` is
    /// executed exactly once per process however many tests race here.
    ///
    /// # Panics
    ///
    /// If the environment failed to build, naming the app and the whole error
    /// chain. The failure is remembered: every later caller panics with the
    /// same message rather than re-running a `setup` that already failed (a
    /// retry would double every side effect the failed attempt had). Use
    /// [`try_get`](Self::try_get) to handle it instead.
    pub async fn get(&'static self) -> A::Env {
        match self.try_get().await {
            Ok(env) => env,
            Err(err) => panic!("{err}"),
        }
    }

    /// [`get`](Self::get) without the panic — for a test that asserts *that*
    /// the environment refuses to build.
    ///
    /// The error is the same for every caller: `setup` runs once, and a failed
    /// environment stays failed for the rest of the process.
    pub async fn try_get(&'static self) -> Result<A::Env, SharedEnvError> {
        let mut rx = self.start().subscribe();
        loop {
            // Clone the slot out before awaiting: a `watch::Ref` must never be
            // held across an await point (it holds the channel's read lock).
            let slot = rx.borrow_and_update().clone();
            if let Some(result) = slot {
                return result;
            }
            rx.changed()
                .await
                .expect("the shared-environment sender lives in a `static`, so it is never dropped");
        }
    }

    /// Start the single `setup` run (idempotent) and return the channel it
    /// publishes to.
    fn start(&'static self) -> &'static watch::Sender<Slot<A::Env>> {
        self.slot.get_or_init(|| {
            let (tx, _rx) = watch::channel(None);
            let publish = tx.clone();
            let init = self.init;
            let handle = shared_runtime().handle();
            // A short-lived thread, not `handle.spawn`: `App::setup()` returns
            // an RPITIT future with no `Send` bound, so it cannot be sent to a
            // runtime — but `Handle::block_on` polls it on *this* thread while
            // the shared runtime is the ambient one, which is what binds the
            // environment's resources (and anything `setup` spawns) to that
            // long-lived runtime. The thread exits as soon as `setup` returns;
            // the runtime it registered everything with does not.
            std::thread::Builder::new()
                .name("r2e-shared-env".to_string())
                .spawn(move || {
                    let result = handle.block_on(async move {
                        match init {
                            Some(make) => make().await,
                            None => A::setup().await,
                        }
                    });
                    // `send_replace`, never `send`: `send` refuses (and
                    // *discards* the value) when the receiver count is zero,
                    // which is the common case here — `setup` can finish before
                    // the caller that started it gets to `subscribe`, and every
                    // waiting test may have been cancelled. Dropping the value
                    // there would leave the slot empty for good and hang every
                    // later caller. `send_replace` always stores and always
                    // notifies.
                    publish.send_replace(Some(result.map_err(|err| SharedEnvError {
                        app: std::any::type_name::<A>(),
                        chain: error_chain(&err),
                    })));
                })
                .expect("failed to spawn the r2e-test shared-environment thread");
            tx
        })
    }
}

impl<A: App + 'static> Default for SharedEnv<A> {
    fn default() -> Self {
        Self::new()
    }
}

impl<A: App> std::fmt::Debug for SharedEnv<A> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = match self.slot.get().map(watch::Sender::borrow) {
            None => "not started",
            Some(slot) => match &*slot {
                None => "building",
                Some(Ok(_)) => "ready",
                Some(Err(_)) => "failed",
            },
        };
        f.debug_struct("SharedEnv")
            .field("app", &std::any::type_name::<A>())
            .field("state", &state)
            .finish()
    }
}

/// A [`SharedEnv`] that failed to build.
///
/// Cloneable (and therefore `'static`-storable) because the one failure is
/// reported to every caller: the underlying [`BootError`] is rendered to its
/// full `caused by:` chain at the moment it happens.
#[derive(Clone, Debug)]
pub struct SharedEnvError {
    app: &'static str,
    chain: String,
}

impl SharedEnvError {
    /// The app whose environment failed, as `std::any::type_name`.
    #[must_use]
    pub fn app(&self) -> &str {
        self.app
    }

    /// The rendered error chain: the boot error, then one `caused by:` line per
    /// `source()` level.
    #[must_use]
    pub fn chain(&self) -> &str {
        &self.chain
    }
}

impl std::fmt::Display for SharedEnvError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "SharedEnv::<{}>::get() failed: {}",
            self.app, self.chain
        )
    }
}

impl std::error::Error for SharedEnvError {}

/// The runtime every shared environment is built and driven on.
///
/// Multi-thread, `enable_all`, parked in a `static` — so it is never dropped
/// and its reactor outlives every per-test runtime. It is created on first use,
/// so a binary that never touches [`SharedEnv`] never pays for it.
fn shared_runtime() -> &'static Runtime {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        RuntimeBuilder::new_multi_thread()
            .thread_name("r2e-shared-env")
            .enable_all()
            .build()
            .expect("failed to build the r2e-test shared-environment runtime")
    })
}

/// A handle to the runtime shared environments are built and driven on.
///
/// For a test that needs to reach the same long-lived reactor directly — e.g.
/// spawning a fixture task that must outlive the test that created it, next to
/// the tasks `setup` spawned. Ordinary tests never need this.
#[must_use]
pub fn shared_env_runtime() -> RuntimeHandle {
    shared_runtime().handle()
}
