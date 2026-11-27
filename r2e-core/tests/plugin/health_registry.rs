//! `HealthRegistry` — the bean `AdvancedHealth` provides so that OTHER plugins
//! can contribute health checks (`type Deps = (HealthRegistry,)`) instead of
//! having to be the health plugin.

use r2e_core::builtins::health::{HealthIndicator, HealthRegistry, HealthStatus};
use r2e_core::builtins::Health;
use r2e_core::http::StatusCode;
use r2e_core::plugin::{Plugin, PluginBuildContext, PluginBuildError};
use r2e_core::AppBuilder;

use crate::support::send_get;

struct NamedCheck {
    name: &'static str,
    up: bool,
}

impl HealthIndicator for NamedCheck {
    fn name(&self) -> &str {
        self.name
    }
    fn check(&self) -> impl std::future::Future<Output = HealthStatus> + Send {
        let up = self.up;
        async move {
            if up {
                HealthStatus::Up
            } else {
                HealthStatus::Down("contributed check is down".into())
            }
        }
    }
}

/// A plugin that contributes a check without being the health plugin.
///
/// One type per install site: a plugin type may only be installed once, so the
/// "several contributors" test below needs two distinct types.
macro_rules! contributor {
    ($Ty:ident) => {
        struct $Ty {
            name: &'static str,
            up: bool,
        }

        impl Plugin for $Ty {
            type Provided = ();
            type Deps = (HealthRegistry,);
            type Config = ();
            type Controllers = ();

            async fn build(
                self,
                (registry,): Self::Deps,
                _config: Option<()>,
                _ctx: &mut PluginBuildContext,
            ) -> Result<(), PluginBuildError> {
                registry.register(NamedCheck {
                    name: self.name,
                    up: self.up,
                });
                Ok(())
            }
        }
    };
}

contributor!(Contributor);
contributor!(OtherContributor);

#[r2e_core::test]
async fn contributed_checks_show_up_on_the_health_endpoint() {
    let router = AppBuilder::new()
        .plugin(Health::builder().build())
        .plugin(Contributor {
            name: "contributed",
            up: true,
        })
        .build_state()
        .await
        .build();

    let (status, body) = send_get(router, "/health").await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["status"], "UP");
    let names: Vec<&str> = json["checks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["contributed"]);
}

#[r2e_core::test]
async fn a_contributed_down_check_fails_readiness() {
    let router = AppBuilder::new()
        .plugin(Health::builder().build())
        .plugin(Contributor {
            name: "broken",
            up: false,
        })
        .build_state()
        .await
        .build();

    let (status, body) = send_get(router.clone(), "/health").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(body.contains("DOWN"));

    // Liveness never depends on the checks.
    let (status, _) = send_get(router, "/health/live").await;
    assert_eq!(status, StatusCode::OK);
}

#[r2e_core::test]
async fn several_plugins_contribute_to_the_same_registry() {
    let router = AppBuilder::new()
        .plugin(
            Health::builder()
                .check(NamedCheck {
                    name: "builder-check",
                    up: true,
                })
                .build(),
        )
        .plugin(Contributor {
            name: "plugin-a",
            up: true,
        })
        .plugin(OtherContributor {
            name: "plugin-b",
            up: true,
        })
        .build_state()
        .await
        .build();

    let (status, body) = send_get(router, "/health").await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    let mut names: Vec<&str> = json["checks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    names.sort_unstable();
    assert_eq!(names, vec!["builder-check", "plugin-a", "plugin-b"]);
}

// ── Registry accessors ───────────────────────────────────────────────────

#[test]
fn a_new_registry_is_empty() {
    let registry = HealthRegistry::new();
    assert!(registry.is_empty());
    assert_eq!(registry.len(), 0);
    assert!(registry.names().is_empty());
    assert_eq!(registry.cache_ttl(), None);
}

#[test]
fn registered_checks_are_counted_and_named_in_order() {
    let registry = HealthRegistry::new();
    registry.register(NamedCheck {
        name: "first",
        up: true,
    });
    assert!(!registry.is_empty());
    assert_eq!(registry.len(), 1);
    registry.register(NamedCheck {
        name: "second",
        up: false,
    });
    assert_eq!(registry.len(), 2);
    assert_eq!(
        registry.names(),
        vec!["first".to_string(), "second".to_string()]
    );

    // Clones share the same registry.
    let clone = registry.clone();
    assert_eq!(clone.len(), 2);
}

#[test]
fn set_cache_ttl_is_read_back_and_last_writer_wins() {
    let registry = HealthRegistry::new();
    registry
        .set_cache_ttl(std::time::Duration::from_secs(5))
        .set_cache_ttl(std::time::Duration::from_secs(7));
    assert_eq!(
        registry.cache_ttl(),
        Some(std::time::Duration::from_secs(7))
    );
    assert_eq!(
        registry.into_state().cache_ttl,
        Some(std::time::Duration::from_secs(7))
    );
}
