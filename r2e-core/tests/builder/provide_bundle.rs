//! `#[derive(ProvideBundle)]` + `AppBuilder::provide_all`: one struct, one
//! provision per field, with an `R2eConfig` field standing in for
//! `override_config`.

use r2e_core::config::{ConfigValue, R2eConfig};
use r2e_core::type_list::BeanAccess;
use r2e_core::prelude::ProvideBundle;
use r2e_core::AppBuilder;

#[derive(Clone, Debug, PartialEq)]
struct Pool(&'static str);

#[derive(Clone, Debug, PartialEq)]
struct S3(u16);

#[derive(Clone, Debug, PartialEq)]
struct Flag(bool);

#[derive(ProvideBundle)]
struct Env {
    pool: Pool,
    s3: S3,
    flag: Flag,
    maybe: Option<S3>,
}

#[derive(ProvideBundle)]
struct EnvWithConfig {
    config: R2eConfig,
    pool: Pool,
}

#[tokio::test]
async fn provide_all_provisions_every_field() {
    let state = AppBuilder::new()
        .provide_all(Env {
            pool: Pool("doris"),
            s3: S3(9000),
            flag: Flag(true),
            maybe: Some(S3(1)),
        })
        .build_state()
        .await;

    assert_eq!(state.state().get::<Pool>(), Pool("doris"));
    assert_eq!(state.state().get::<S3>(), S3(9000));
    assert_eq!(state.state().get::<Flag>(), Flag(true));
    // `Option<T>` is a first-class bean type: provided as-is, never unwrapped.
    assert_eq!(state.state().get::<Option<S3>>(), Some(S3(1)));
}

#[tokio::test]
async fn none_option_field_is_still_a_provision() {
    let state = AppBuilder::new()
        .provide_all(Env {
            pool: Pool("p"),
            s3: S3(1),
            flag: Flag(false),
            maybe: None,
        })
        .build_state()
        .await;

    assert_eq!(state.state().get::<Option<S3>>(), None);
}

#[tokio::test]
async fn r2e_config_field_acts_as_override_config() {
    let mut config = R2eConfig::empty();
    config.set("app.name", ConfigValue::from("bundled"));

    let state = AppBuilder::new()
        .provide_all(EnvWithConfig {
            config,
            pool: Pool("p"),
        })
        // The config field removed the "override_config BEFORE load_config"
        // ordering constraint: `provide_all` is just the first link of the
        // chain.
        .load_config::<()>()
        .build_state()
        .await;

    let loaded = state.state().get::<R2eConfig>();
    assert_eq!(loaded.try_get::<String>("app.name").as_deref(), Some("bundled"));
    assert_eq!(state.state().get::<Pool>(), Pool("p"));
}
