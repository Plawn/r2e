//! `AppBuilder::provide_config`: hand over a typed settings struct built in
//! code and get the same bean set `load_config::<C>()` provides — the parent
//! **and** every nested `#[config(section)]` child — with no YAML on disk.

use r2e_core::type_list::BeanAccess;
use r2e_core::AppBuilder;
use std::sync::Arc;

#[derive(r2e_macros::ConfigProperties, Clone, Debug, PartialEq)]
pub struct DorisSettings {
    pub url: String,
    #[config(default = 5)]
    pub pool_size: i64,
}

#[derive(r2e_macros::ConfigProperties, Clone, Debug, PartialEq)]
pub struct S3Settings {
    pub bucket: String,
}

#[derive(r2e_macros::ConfigProperties, Clone, Debug, PartialEq)]
pub struct InfraSettings {
    pub name: String,
    #[config(section)]
    pub doris: DorisSettings,
    #[config(section)]
    pub s3: S3Settings,
}

fn settings() -> InfraSettings {
    InfraSettings {
        name: "catalog".into(),
        doris: DorisSettings {
            url: "doris://localhost".into(),
            pool_size: 11,
        },
        s3: S3Settings {
            bucket: "icons".into(),
        },
    }
}

#[tokio::test]
async fn provide_config_provides_parent_and_children() {
    let state = AppBuilder::new()
        .provide_config(settings())
        .build_state()
        .await;

    assert_eq!(state.state().get::<InfraSettings>().name, "catalog");
    // The children are beans in their own right — no producer boilerplate.
    assert_eq!(state.state().get::<DorisSettings>().pool_size, 11);
    assert_eq!(state.state().get::<S3Settings>().bucket, "icons");
}

// A child section reachable through the graph the way an app consumes it:
// a bean that injects the child and republishes it as `Arc<Child>`.
#[derive(Clone)]
struct DorisPool(Arc<DorisSettings>);

#[r2e_macros::bean]
impl DorisPool {
    fn new(#[inject] doris: DorisSettings) -> Self {
        Self(Arc::new(doris))
    }
}

#[tokio::test]
async fn child_section_is_injectable_without_load_config() {
    let state = AppBuilder::new()
        .provide_config(settings())
        .register::<DorisPool>()
        .build_state()
        .await;

    let pool = state.state().get::<DorisPool>();
    assert_eq!(pool.0.url, "doris://localhost");
    assert_eq!(pool.0.pool_size, 11);
}

// Nested two levels deep: `register_children` recurses, so a grandchild
// section is a bean too.
#[derive(r2e_macros::ConfigProperties, Clone, Debug, PartialEq)]
pub struct TlsSettings {
    pub ca: String,
}

#[derive(r2e_macros::ConfigProperties, Clone, Debug, PartialEq)]
pub struct BrokerSettings {
    pub host: String,
    #[config(section)]
    pub tls: TlsSettings,
}

#[derive(r2e_macros::ConfigProperties, Clone, Debug, PartialEq)]
pub struct RootSettings {
    #[config(section)]
    pub broker: BrokerSettings,
}

#[tokio::test]
async fn provide_config_registers_grandchildren() {
    let state = AppBuilder::new()
        .provide_config(RootSettings {
            broker: BrokerSettings {
                host: "kafka:9092".into(),
                tls: TlsSettings { ca: "ca.pem".into() },
            },
        })
        .build_state()
        .await;

    assert_eq!(state.state().get::<BrokerSettings>().host, "kafka:9092");
    assert_eq!(state.state().get::<TlsSettings>().ca, "ca.pem");
}
