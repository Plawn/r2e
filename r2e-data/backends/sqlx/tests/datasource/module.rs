//! A feature module **bringing** the `SqlxDataSource` plugin: the datasource is
//! part of the module's declaration, not of the app assembly.
//!
//! The plugin's `DbPool<Sqlite>` is app-global (as every plugin bean is), so it
//! is in the module's local scope — a module-private repository depends on it —
//! and no `exports(DbPool<Sqlite>)` entry is needed. What the module *does*
//! export is its own repository.

use r2e_core::di::module::FeatureModule;
use r2e_core::prelude::*;
use r2e_core::type_list::BeanAccess;
use r2e_core::R2eConfig;
use r2e_data_sqlx::{DbPool, SqlxDataSource};
use sqlx::Sqlite;

use crate::support::{cleanup_sqlite_file, sqlite_file_url};

/// Module-private repository over the brought plugin's pool.
#[derive(Clone)]
pub struct ItemRepo {
    pool: DbPool<Sqlite>,
}

#[bean]
impl ItemRepo {
    fn new(pool: DbPool<Sqlite>) -> Self {
        Self { pool }
    }

    async fn one(&self) -> i64 {
        sqlx::query_scalar("SELECT 1")
            .fetch_one(&self.pool.current())
            .await
            .unwrap()
    }
}

#[module(
    providers(ItemRepo),
    exports(ItemRepo),
    plugins(SqlxDataSource<Sqlite> = SqlxDataSource::<Sqlite>::new())
)]
pub struct DataModule;

/// `register_module::<DataModule>()` alone connects the datasource: no
/// `.plugin(SqlxDataSource…)` in the app assembly.
#[tokio::test]
async fn module_brings_the_datasource_plugin() {
    let url = sqlite_file_url("module-brings-ds");
    let mut config = R2eConfig::empty();
    config.set("datasource.url", url.clone().into());

    let app = AppBuilder::new()
        .override_config(config)
        .load_config::<()>()
        .register_module::<DataModule>()
        .build_state()
        .await;

    // The plugin's bean joined the app-global provision list without being
    // exported, and the module's exported repository can use it.
    let pool = app.state().get::<DbPool<Sqlite>>();
    assert!(!pool.current().is_closed());
    assert_eq!(app.state().get::<ItemRepo>().one().await, 1);

    cleanup_sqlite_file(&url);
}

/// The brought plugin is declared at the type level too: `Plugins` carries it,
/// so it grows the module's scope and the app-global provision list.
#[test]
fn brought_plugin_is_part_of_the_module_declaration() {
    fn assert_same<A: 'static, B: 'static>() {
        assert_eq!(
            std::any::TypeId::of::<A>(),
            std::any::TypeId::of::<B>(),
            "{} != {}",
            std::any::type_name::<A>(),
            std::any::type_name::<B>()
        );
    }
    assert_same::<<DataModule as FeatureModule>::Plugins, (SqlxDataSource<Sqlite>,)>();
}
