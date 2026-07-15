//! Shared helpers for the dev-service containers.
//!
//! Cleanup and reuse are driven by testcontainers' [`ReuseDirective`] plus a
//! stable, deterministic container name. Two modes coexist:
//!
//! * **Reuse** (used by `shared()`): `ReuseDirective::Always` with a stable
//!   name (`r2e-dev<service>-<tag>`). Every test process attaches to the *same*
//!   container, and testcontainers declines to reap a reuse container on drop —
//!   so exactly one warm container survives across processes and runs. This is
//!   what keeps test suites from spawning (and leaking) one container per test
//!   binary.
//! * **Isolated** (used by `start()` / `start_with_tag()`): `ReuseDirective::Never`.
//!   The container is bound to the returned handle and removed by testcontainers'
//!   `Drop` impl when the handle goes out of scope — unless
//!   [`R2E_DEVSERVICES_KEEP`](KEEP_ENV) is set, in which case it is kept alive
//!   for post-mortem inspection.

use std::time::{Duration, Instant};

use testcontainers::core::ContainerRequest;
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, Image};

/// Set to a truthy value (`1`/`true`/`yes`/`on`) to keep one-off `start()`
/// containers alive after their handle drops, instead of auto-removing them.
///
/// This does **not** affect `shared()`: the shared container is reused and
/// deliberately persists across processes regardless of this flag.
pub(crate) const KEEP_ENV: &str = "R2E_DEVSERVICES_KEEP";

/// Whether one-off containers should be kept alive after their handle drops.
pub(crate) fn keep_enabled() -> bool {
    matches!(
        std::env::var(KEEP_ENV)
            .ok()
            .as_deref()
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("1" | "true" | "yes" | "on")
    )
}

/// Build the stable, cross-process container name for a shared dev service,
/// e.g. `r2e-devpostgres-16-alpine`.
///
/// Names are derived from the image tag so that different tags get different
/// (still deterministic) containers, and Docker-illegal characters are folded
/// to `-` so any tag yields a valid name.
pub(crate) fn shared_name(service: &str, tag: &str) -> String {
    format!("r2e-dev{service}-{}", sanitize(tag))
}

/// Fold a string into the Docker-legal container-name alphabet
/// (`[a-zA-Z0-9_.-]`).
pub(crate) fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// Start a container, retrying only on a name conflict.
///
/// A reuse request first looks up the container by name; if it does not exist
/// it is created. When several test binaries cold-start at once they can race:
/// the loser's `create` fails with a Docker `409 Conflict` ("name already in
/// use"). We simply retry — the next lookup finds the winner's container and
/// attaches to it. Any *other* error (e.g. Docker not running) fails fast, as
/// before.
///
/// `make` rebuilds the request each attempt because `start()` consumes it.
pub(crate) async fn start_with_retry<I, F>(what: &str, make: F) -> ContainerAsync<I>
where
    I: Image,
    F: Fn() -> ContainerRequest<I>,
{
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match make().start().await {
            Ok(container) => return container,
            Err(e) => {
                let msg = e.to_string();
                let is_name_conflict = msg.contains("already in use")
                    || msg.contains("Conflict")
                    || msg.contains("409");
                if !is_name_conflict {
                    panic!("failed to start the {what} dev service — is Docker running? ({msg})");
                }
                if Instant::now() >= deadline {
                    panic!("{what} dev service still contended after 30s: {msg}");
                }
                tokio::time::sleep(Duration::from_millis(300)).await;
            }
        }
    }
}

/// Wait until `host:port` accepts TCP connections (up to 60s).
///
/// On the reuse path testcontainers returns as soon as an existing container is
/// *running*, without re-checking readiness — so a process that attaches while
/// the creator is still running `initdb` could otherwise observe a not-yet-ready
/// service. A best-effort TCP probe closes that window. Runs on a blocking
/// worker so it never stalls the async runtime.
pub(crate) async fn wait_tcp_ready(host: String, port: u16) {
    let _ = tokio::task::spawn_blocking(move || {
        use std::net::{TcpStream, ToSocketAddrs};

        let host = if host == "localhost" {
            "127.0.0.1".to_string()
        } else {
            host
        };
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            if let Ok(mut addrs) = (host.as_str(), port).to_socket_addrs() {
                if let Some(addr) = addrs.next() {
                    if TcpStream::connect_timeout(&addr, Duration::from_secs(2)).is_ok() {
                        return;
                    }
                }
            }
            if Instant::now() >= deadline {
                return;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    })
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_keeps_legal_chars_and_folds_the_rest() {
        assert_eq!(sanitize("16-alpine"), "16-alpine");
        assert_eq!(sanitize("7.2.4"), "7.2.4");
        assert_eq!(sanitize("my/tag:weird"), "my-tag-weird");
    }

    #[test]
    fn shared_name_is_stable_and_deterministic() {
        assert_eq!(shared_name("postgres", "16-alpine"), "r2e-devpostgres-16-alpine");
        assert_eq!(shared_name("redis", "7-alpine"), "r2e-devredis-7-alpine");
        // Same inputs → same name (cross-process reuse relies on this).
        assert_eq!(
            shared_name("postgres", "16-alpine"),
            shared_name("postgres", "16-alpine")
        );
    }
}
