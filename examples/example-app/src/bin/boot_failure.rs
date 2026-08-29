//! A deliberately failing boot, driven by `tests/app/boot_failure.rs`.
//!
//! It is assembled exactly like a production entry point — `launch!` +
//! `exit_on_boot_error`, which is what `app_main!` expands to — so the test can
//! assert the operational contract of a boot failure from the outside: exit
//! status 1, and ONE `error:` line followed by the cause chain (no panic, no
//! backtrace, no half-started server).

use r2e::{App, AppBuilder, BootError, BootableApp};

/// A setup failure with a `source()`, the shape a real driver error has.
#[derive(Debug)]
struct MissingSecret(std::io::Error);

impl std::fmt::Display for MissingSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "cannot read the database secret")
    }
}

impl std::error::Error for MissingSecret {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

struct FailingApp;

impl App for FailingApp {
    type Env = ();

    async fn setup() -> Result<(), BootError> {
        Err(Box::new(MissingSecret(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "/run/secrets/db-url: no such file",
        ))))
    }

    async fn build(b: AppBuilder, _env: ()) -> Result<impl BootableApp, BootError> {
        Ok(b.build_state().await)
    }
}

#[r2e::main]
async fn main() {
    r2e::exit_on_boot_error(r2e::launch!(FailingApp).await);
}
