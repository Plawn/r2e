use std::sync::OnceLock;
use std::time::{Duration, Instant};

use testcontainers::bollard::models::ContainerSummaryStateEnum;
use testcontainers::bollard::query_parameters::{
    ListContainersOptionsBuilder, RemoveContainerOptionsBuilder,
};
use testcontainers::core::client::{docker_client_instance, ClientError};
use testcontainers::core::error::TestcontainersError;
use testcontainers::core::ContainerRequest;
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, Image, ImageExt};

pub(crate) const MANAGED_LABEL: &str = "dev.r2e.devservices.managed";
pub(crate) const SCOPE_LABEL: &str = "dev.r2e.devservices.scope";
const SERVICE_LABEL: &str = "dev.r2e.devservices.service";
const CONFIG_LABEL: &str = "dev.r2e.devservices.config";
const MODE_LABEL: &str = "dev.r2e.devservices.mode";

/// Disable Ryuk and automatic garbage collection for post-mortem inspection.
pub(crate) const KEEP_ENV: &str = "R2E_DEVSERVICES_KEEP";

/// Stable identity attached to a cross-process shared container.
pub(crate) struct SharedIdentity {
    service: &'static str,
    name: String,
    fingerprint: String,
}

impl SharedIdentity {
    /// Build an identity from every input that affects the service configuration.
    pub(crate) fn new(service: &'static str, configuration: &str) -> Self {
        let fingerprint = fingerprint(configuration);
        let name = format!(
            "r2e-devservices-{}-{service}-{fingerprint}",
            session_scope()
        );
        Self {
            service,
            name,
            fingerprint,
        }
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    /// Add the labels used for reuse matching, Ryuk, and fallback cleanup.
    pub(crate) fn label<I: Image>(&self, request: ContainerRequest<I>) -> ContainerRequest<I> {
        managed_labels(request, self.service, &self.fingerprint, "shared")
    }
}

/// Label an isolated container so Ryuk can reap it after an abnormal exit.
pub(crate) fn label_isolated<I: Image>(
    request: ContainerRequest<I>,
    service: &'static str,
    configuration: &str,
) -> ContainerRequest<I> {
    managed_labels(request, service, &fingerprint(configuration), "isolated")
}

fn managed_labels<I: Image>(
    request: ContainerRequest<I>,
    service: &'static str,
    configuration: &str,
    mode: &'static str,
) -> ContainerRequest<I> {
    request
        .with_label(MANAGED_LABEL, "true")
        .with_label(SCOPE_LABEL, session_scope())
        .with_label(SERVICE_LABEL, service)
        .with_label(CONFIG_LABEL, configuration)
        .with_label(MODE_LABEL, mode)
}

/// Stable session shared by every test binary launched from the same workspace.
pub(crate) fn session_scope() -> &'static str {
    static SCOPE: OnceLock<String> = OnceLock::new();
    SCOPE.get_or_init(|| {
        let source = std::env::var("R2E_DEVSERVICES_SESSION").unwrap_or_else(|_| {
            std::env::current_dir()
                .ok()
                .and_then(|path| path.canonicalize().ok().or(Some(path)))
                .map(|path| path.to_string_lossy().into_owned())
                .unwrap_or_else(|| "r2e-default".to_string())
        });
        fingerprint(&source)
    })
}

pub(crate) fn keep_enabled() -> bool {
    truthy_env(KEEP_ENV)
}

pub(crate) fn truthy_env(name: &str) -> bool {
    std::env::var(name).ok().is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

/// Start a container and recover from concurrent creators losing Docker's name race.
pub(crate) async fn start_with_retry<I, F>(what: &str, make: F) -> ContainerAsync<I>
where
    I: Image,
    F: Fn() -> ContainerRequest<I>,
{
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match make().start().await {
            Ok(container) => return container,
            Err(error) if is_name_conflict(&error) && Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            Err(error) => {
                panic!("failed to start the {what} dev service — is Docker running?: {error}")
            }
        }
    }
}

fn is_name_conflict(error: &TestcontainersError) -> bool {
    matches!(
        error,
        TestcontainersError::Client(ClientError::CreateContainer(
            testcontainers::bollard::errors::Error::DockerResponseServerError {
                status_code: 409,
                ..
            }
        ))
    )
}

/// Fallback cleanup for resources left behind before Ryuk was available.
///
/// Ryuk is the primary lifecycle owner. This only removes stopped containers,
/// same-scope duplicates, and containers created by the pre-Ryuk implementation.
pub(crate) async fn cleanup(identity: &SharedIdentity) {
    if keep_enabled() {
        return;
    }

    let Ok(docker) = docker_client_instance().await else {
        return;
    };
    let filters = std::collections::HashMap::from([(
        "label".to_string(),
        vec![
            format!("{MANAGED_LABEL}=true"),
            format!("{SERVICE_LABEL}={}", identity.service),
        ],
    )]);
    let options = ListContainersOptionsBuilder::new()
        .all(true)
        .filters(&filters)
        .build();
    let Ok(containers) = docker.list_containers(Some(options)).await else {
        return;
    };

    let remove_options = RemoveContainerOptionsBuilder::new().force(true).build();
    for container in containers {
        let Some(id) = container.id.as_deref() else {
            continue;
        };
        let labels = container.labels.as_ref();
        let container_scope = labels.and_then(|labels| labels.get(SCOPE_LABEL));
        let same_scope = container_scope.is_some_and(|value| value == session_scope());
        let legacy = container_scope.is_none();
        let same_configuration = labels
            .and_then(|labels| labels.get(CONFIG_LABEL))
            .is_some_and(|value| value == &identity.fingerprint);
        let current_name = container.names.as_ref().is_some_and(|names| {
            names
                .iter()
                .any(|name| name.trim_start_matches('/') == identity.name)
        });
        let active = matches!(
            container.state,
            Some(ContainerSummaryStateEnum::RUNNING | ContainerSummaryStateEnum::RESTARTING)
        );

        if !active || legacy || (same_scope && same_configuration && !current_name) {
            let _ = docker
                .remove_container(id, Some(remove_options.clone()))
                .await;
        }
    }
}

/// Wait until the published service port accepts a connection.
pub(crate) async fn wait_tcp_ready(host: &str, port: u16, what: &str) {
    let host = host.to_string();
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let probe_host = host.clone();
        let reachable = tokio::task::spawn_blocking(move || {
            use std::net::{TcpStream, ToSocketAddrs};

            (probe_host.as_str(), port)
                .to_socket_addrs()
                .ok()
                .into_iter()
                .flatten()
                .any(|address| TcpStream::connect_timeout(&address, Duration::from_secs(2)).is_ok())
        })
        .await
        .unwrap_or(false);

        if reachable {
            return;
        }
        if Instant::now() >= deadline {
            panic!("{what} dev service did not become reachable at {host}:{port} within 60s");
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

fn fingerprint(value: &str) -> String {
    format!("{:016x}", fnv1a(value.as_bytes()))
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_is_stable_and_configuration_sensitive() {
        let first = SharedIdentity::new("postgres", "postgres:16-alpine;port=5432");
        let again = SharedIdentity::new("postgres", "postgres:16-alpine;port=5432");
        let changed = SharedIdentity::new("postgres", "postgres:17-alpine;port=5432");

        assert_eq!(first.name, again.name);
        assert_eq!(first.fingerprint, again.fingerprint);
        assert_ne!(first.name, changed.name);
        assert!(first.name.contains("-postgres-"));
    }

    #[test]
    fn fingerprints_are_stable() {
        assert_eq!(fingerprint("r2e"), fingerprint("r2e"));
        assert_ne!(fingerprint("r2e"), fingerprint("other"));
    }
}
