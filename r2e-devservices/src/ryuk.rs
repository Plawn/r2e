use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use testcontainers::core::{IntoContainerPort, Mount, WaitFor};
use testcontainers::{GenericImage, ImageExt, ReuseDirective};
use tokio::sync::OnceCell;

use crate::common;

const IMAGE: &str = "testcontainers/ryuk";
const TAG: &str = "0.14.0";
const CONTAINER_PORT: u16 = 8080;
const RYUK_LABEL: &str = "org.testcontainers.ryuk";
const SOCKET_TARGET: &str = "/var/run/docker.sock";

/// A process-wide TCP lease. The OS closes it even when the process is killed,
/// which lets Ryuk detect the end of the complete multi-process test session.
struct RyukLease {
    _stream: TcpStream,
}

pub(crate) async fn ensure_lease() {
    if common::keep_enabled() {
        return;
    }

    static LEASE: OnceCell<RyukLease> = OnceCell::const_new();
    LEASE.get_or_init(start).await;
}

async fn start() -> RyukLease {
    let socket = docker_socket().unwrap_or_else(|error| panic!("cannot start Ryuk: {error}"));
    let scope = common::session_scope();
    let name = format!("r2e-devservices-ryuk-{scope}");
    let reconnect_timeout = std::env::var("R2E_DEVSERVICES_RYUK_RECONNECTION_TIMEOUT")
        .unwrap_or_else(|_| "10s".to_string());
    let privileged = common::truthy_env("R2E_DEVSERVICES_RYUK_PRIVILEGED");

    let filter = format!("label={}={scope}\n", common::SCOPE_LABEL);
    let deadline = Instant::now() + Duration::from_secs(30);

    loop {
        let container = common::start_with_retry("Ryuk", || {
            GenericImage::new(IMAGE, TAG)
                .with_exposed_port(CONTAINER_PORT.tcp())
                .with_wait_for(WaitFor::message_on_stdout("Started"))
                .with_container_name(&name)
                .with_mount(Mount::bind_mount(
                    socket.to_string_lossy().into_owned(),
                    SOCKET_TARGET,
                ))
                .with_env_var("RYUK_RECONNECTION_TIMEOUT", &reconnect_timeout)
                .with_label(RYUK_LABEL, "true")
                .with_label(common::SCOPE_LABEL, scope)
                .with_privileged(privileged)
                .with_host_config_modifier(|host_config| host_config.auto_remove = Some(true))
                .with_reuse(ReuseDirective::Always)
        })
        .await;

        let endpoint = async {
            let host = container.get_host().await.ok()?.to_string();
            let port = container.get_host_port_ipv4(CONTAINER_PORT).await.ok()?;
            Some((host, port))
        }
        .await;

        let attempt_error = if let Some((host, port)) = endpoint {
            let filter = filter.clone();
            match tokio::task::spawn_blocking(move || {
                connect(&host, port, &filter, Duration::from_secs(3))
            })
            .await
            .expect("Ryuk connection worker panicked")
            {
                Ok(stream) => {
                    // The reusable handle may be dropped: Ryuk owns its own
                    // lifetime and Docker auto-removes it after it exits.
                    drop(container);
                    return RyukLease { _stream: stream };
                }
                Err(error) => error,
            }
        } else {
            "the Ryuk container disappeared before port resolution".to_string()
        };

        drop(container);
        if Instant::now() >= deadline {
            panic!("failed to establish the Ryuk lease within 30s: {attempt_error}");
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

fn connect(host: &str, port: u16, filter: &str, timeout: Duration) -> Result<TcpStream, String> {
    let deadline = Instant::now() + timeout;
    let mut last_error = String::new();

    while Instant::now() < deadline {
        let addresses = (host, port)
            .to_socket_addrs()
            .map_err(|error| format!("cannot resolve {host}:{port}: {error}"))?;
        for address in addresses {
            match TcpStream::connect_timeout(&address, Duration::from_secs(2)) {
                Ok(mut stream) => {
                    stream
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .map_err(|error| error.to_string())?;
                    stream
                        .set_write_timeout(Some(Duration::from_secs(5)))
                        .map_err(|error| error.to_string())?;
                    if let Err(error) = stream.write_all(filter.as_bytes()) {
                        last_error = error.to_string();
                        continue;
                    }
                    let mut ack = [0_u8; 4];
                    if let Err(error) = stream.read_exact(&mut ack) {
                        last_error = error.to_string();
                        continue;
                    }
                    if &ack != b"ACK\n" {
                        last_error = format!("unexpected response: {ack:?}");
                        continue;
                    }
                    stream
                        .set_read_timeout(None)
                        .map_err(|error| error.to_string())?;
                    stream
                        .set_write_timeout(None)
                        .map_err(|error| error.to_string())?;
                    return Ok(stream);
                }
                Err(error) => last_error = error.to_string(),
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }

    Err(format!("Ryuk at {host}:{port} was not ready: {last_error}"))
}

fn docker_socket() -> Result<PathBuf, String> {
    for variable in [
        "R2E_DEVSERVICES_DOCKER_SOCKET",
        "TESTCONTAINERS_DOCKER_SOCKET_OVERRIDE",
    ] {
        if let Some(path) = std::env::var_os(variable).map(PathBuf::from) {
            return existing_socket(path, variable);
        }
    }

    if let Ok(host) = std::env::var("DOCKER_HOST") {
        if let Some(path) = host.strip_prefix("unix://") {
            return existing_socket(PathBuf::from(path), "DOCKER_HOST");
        }
        if !host.trim().is_empty() {
            return Err(format!(
                "Ryuk currently requires a Unix Docker socket, but DOCKER_HOST is {host:?}; set R2E_DEVSERVICES_DOCKER_SOCKET"
            ));
        }
    }

    let mut candidates = vec![PathBuf::from("/var/run/docker.sock")];
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
        candidates.push(PathBuf::from(runtime).join(".docker/run/docker.sock"));
    }
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        candidates.push(home.join(".docker/run/docker.sock"));
        candidates.push(home.join(".docker/desktop/docker.sock"));
    }

    candidates
        .into_iter()
        .find(|path| path.exists())
        .ok_or_else(|| {
            "Docker socket not found; set R2E_DEVSERVICES_DOCKER_SOCKET to its host path"
                .to_string()
        })
}

fn existing_socket(path: PathBuf, source: &str) -> Result<PathBuf, String> {
    if path.exists() {
        Ok(path)
    } else {
        Err(format!(
            "Docker socket from {source} does not exist: {}",
            path.display()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_filter_has_the_expected_shape() {
        let filter = format!("label={}={}\n", common::SCOPE_LABEL, "abc123");
        assert_eq!(filter, "label=dev.r2e.devservices.scope=abc123\n");
    }
}
