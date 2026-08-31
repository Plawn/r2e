//! A boot that fails *after* `App::setup` — inside `App::build`, while the
//! graph is being assembled. Driven by `tests/app/boot_failure.rs`.
//!
//! `boot_failure.rs` covers the failure an app raises itself (`setup`
//! returning `Err`). This one covers the two the framework raises on the app's
//! behalf, both reached through `?` on `try_build_state()`:
//!
//! * `producer` — a bean constructor that fails (a pool that will not connect);
//! * `config`   — a requested config file that is not there.
//!
//! Same entry point as any R2E binary (`launch!` + `exit_on_boot_error`), so
//! the test asserts the same operational contract: exit 1, one `error:` line,
//! no panic.

use r2e::prelude::*;
use r2e::{App, AppBuilder, BootError, BootableApp};

#[derive(Debug)]
struct ConnectFailed(&'static str);

impl std::fmt::Display for ConnectFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "could not connect to {}", self.0)
    }
}

impl std::error::Error for ConnectFailed {}

#[derive(Clone)]
struct DbPool;

fn failure_kind() -> String {
    std::env::var("R2E_BOOT_FAILURE_KIND").unwrap_or_default()
}

#[producer]
async fn connect_pool() -> Result<DbPool, ConnectFailed> {
    // In `config` mode the graph is sound: the boot must fail on the config,
    // before any bean is built.
    if failure_kind() == "config" {
        Ok(DbPool)
    } else {
        Err(ConnectFailed("postgres://db:5432/app"))
    }
}

struct FailingApp;

impl App for FailingApp {
    type Env = ();

    async fn setup() -> Result<(), BootError> {
        Ok(())
    }

    async fn build(b: AppBuilder, _env: ()) -> Result<impl BootableApp, BootError> {
        // Both arms build the same graph: `setup` succeeded, so whatever
        // fails here fails on the framework's side of the boot.
        let b = b.register::<ConnectPool>();
        let b = if failure_kind() == "config" {
            b.with_config_file("no-such-application-984.yaml")
        } else {
            b
        };
        Ok(b.load_config::<()>().try_build_state().await?)
    }
}

#[r2e::main]
async fn main() {
    r2e::exit_on_boot_error(r2e::launch!(FailingApp).await);
}

/// Compile-only coverage for the `tracing = false` arm of `launch!` — the arm
/// `app_main!(App, tracing = false)` expands to. Never called: the point is
/// that the knob keeps type-checking on both the plain and the `dev-reload`
/// expansion.
#[allow(dead_code)]
async fn launch_without_tracing() -> Result<(), BootError> {
    r2e::launch!(FailingApp, tracing = false).await
}
