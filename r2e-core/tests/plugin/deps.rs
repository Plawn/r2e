//! `Deps` resolution: dependencies arrive fully constructed before `build`,
//! as real topological edges in the bean graph.

use std::any::TypeId;

use r2e_core::beans::{Bean, BeanContext, BeanRegistry, Registrable};
use r2e_core::http::routing::get;
use r2e_core::http::StatusCode;
use r2e_core::plugin::{Plugin, PluginBuildContext, PluginBuildError};
use r2e_core::type_list::BeanAccess;
use r2e_core::{AppBuilder, TNil};

use crate::fixtures::{Alpha, Beta, StoredData};
use crate::support::send_get as get_route;

/// A **factory-built** bean: constructed by the bean graph, never handed to
/// `.provide()`. Its `build` stamps a recognizable value so a test can tell the
/// factory-built instance apart from a hand-provided one.
#[derive(Clone, Debug, PartialEq)]
struct FactoryBean(u32);

impl Bean for FactoryBean {
    type Error = ::std::convert::Infallible;
    type Deps = TNil;
    fn dependencies() -> Vec<(TypeId, &'static str)> {
        vec![]
    }
    fn build(_ctx: &BeanContext) -> ::std::result::Result<Self, Self::Error> {
        ::std::result::Result::Ok(FactoryBean(99))
    }
}

impl Registrable for FactoryBean {
    type Provided = Self;
    type Deps = TNil;
    fn register_into(registry: &mut BeanRegistry) {
        registry.register::<Self>();
    }
}

/// `Deps = (Alpha,)` — echoes the dependency it received into its own
/// provided bean, so tests can assert on the exact value `build` saw.
struct AlphaEcho;

impl Plugin for AlphaEcho {
    type Provided = (Beta,);
    type Deps = (Alpha,);
    type Config = ();
    type Controllers = ();

    async fn build(
        self,
        (alpha,): (Alpha,),
        _config: Option<()>,
        _ctx: &mut PluginBuildContext,
    ) -> Result<(Beta,), PluginBuildError> {
        Ok((Beta(format!("alpha-{}", alpha.0)),))
    }
}

#[r2e_core::test]
async fn deps_resolve_bean_provided_after_the_plugin() {
    // `Alpha` is provided AFTER `.plugin()` — `Deps` is not checked at the
    // call site, only against the final provision list at `build_state()`.
    let app = AppBuilder::new()
        .plugin(AlphaEcho)
        .provide(Alpha(7))
        .build_state()
        .await;
    assert_eq!(app.state().get::<Beta>(), Beta("alpha-7".into()));
}

/// `Deps = (FactoryBean,)` — a bean only the graph can build, registered
/// *after* this plugin.
struct FactoryEcho;

impl Plugin for FactoryEcho {
    type Provided = (Beta,);
    type Deps = (FactoryBean,);
    type Config = ();
    type Controllers = ();

    async fn build(
        self,
        (fb,): (FactoryBean,),
        _config: Option<()>,
        _ctx: &mut PluginBuildContext,
    ) -> Result<(Beta,), PluginBuildError> {
        Ok((Beta(format!("factory-{}", fb.0)),))
    }
}

#[r2e_core::test]
async fn deps_resolve_factory_built_bean_registered_after_plugin() {
    let app = AppBuilder::new()
        .plugin(FactoryEcho)
        .register::<FactoryBean>()
        .build_state()
        .await;
    // build saw the factory-built instance (build() stamped 99)…
    assert_eq!(app.state().get::<Beta>(), Beta("factory-99".into()));
    // …and the same bean is a normal member of the resolved graph.
    assert_eq!(
        app.bean_context().as_ref().get::<FactoryBean>(),
        FactoryBean(99)
    );
}

/// The "producer" side of the cross-plugin case: provides `Alpha` from build.
struct AlphaProviderPlugin;

impl Plugin for AlphaProviderPlugin {
    type Provided = (Alpha,);
    type Deps = ();
    type Config = ();
    type Controllers = ();

    async fn build(
        self,
        _deps: (),
        _config: Option<()>,
        _ctx: &mut PluginBuildContext,
    ) -> Result<(Alpha,), PluginBuildError> {
        Ok((Alpha(11),))
    }
}

#[r2e_core::test]
async fn deps_resolve_bean_provided_by_another_plugin_in_either_order() {
    // Producer installed first.
    let app = AppBuilder::new()
        .plugin(AlphaProviderPlugin)
        .plugin(AlphaEcho)
        .build_state()
        .await;
    assert_eq!(app.state().get::<Beta>(), Beta("alpha-11".into()));

    // Consumer installed first: builds run in topological order, not install
    // order, so the result is identical.
    let app = AppBuilder::new()
        .plugin(AlphaEcho)
        .plugin(AlphaProviderPlugin)
        .build_state()
        .await;
    assert_eq!(app.state().get::<Beta>(), Beta("alpha-11".into()));
}

#[r2e_core::test]
async fn deps_see_pinned_override_not_the_producing_plugin() {
    // The factory-first contract: EVERYTHING reads the graph. Pinning Alpha
    // replaces the producer plugin's bean, and every consumer — plugin `Deps`
    // included — sees the override.
    let app = AppBuilder::new()
        .override_bean(Alpha(99))
        .plugin(AlphaProviderPlugin) // all-pinned → its build is skipped
        .plugin(AlphaEcho)
        .build_state()
        .await;
    assert_eq!(app.state().get::<Alpha>(), Alpha(99));
    assert_eq!(app.state().get::<Beta>(), Beta("alpha-99".into()));
}

/// The reverse edge: an ordinary `.register()`-ed bean depending on a
/// plugin-provided bean. Its factory reads `Alpha` from the graph, so the
/// plugin's build must have run first.
#[derive(Clone, Debug, PartialEq)]
struct NeedsAlpha(u32);

impl Bean for NeedsAlpha {
    type Error = ::std::convert::Infallible;
    type Deps = TNil;
    fn dependencies() -> Vec<(TypeId, &'static str)> {
        vec![(TypeId::of::<Alpha>(), "Alpha")]
    }
    fn build(ctx: &BeanContext) -> ::std::result::Result<Self, Self::Error> {
        ::std::result::Result::Ok(NeedsAlpha(ctx.get::<Alpha>().0 + 1))
    }
}

impl Registrable for NeedsAlpha {
    type Provided = Self;
    type Deps = TNil;
    fn register_into(registry: &mut BeanRegistry) {
        registry.register::<Self>();
    }
}

#[r2e_core::test]
async fn registered_bean_can_depend_on_plugin_provided_bean() {
    let app = AppBuilder::new()
        .register::<NeedsAlpha>()
        .plugin(AlphaProviderPlugin)
        .build_state()
        .await;
    assert_eq!(app.state().get::<NeedsAlpha>(), NeedsAlpha(12));
}

/// `build` reaching for the effect surface (store_data + a layer) with values
/// computed from its deps.
struct EffectfulEcho;

impl Plugin for EffectfulEcho {
    type Provided = ();
    type Deps = (Alpha,);
    type Config = ();
    type Controllers = ();

    async fn build(
        self,
        (alpha,): (Alpha,),
        _config: Option<()>,
        ctx: &mut PluginBuildContext,
    ) -> Result<(), PluginBuildError> {
        let v = alpha.0;
        ctx.store_data(StoredData(v));
        ctx.add_layer(move |router| {
            router.route("/late", get(move || async move { format!("late-{v}") }))
        });
        Ok(())
    }
}

#[r2e_core::test]
async fn build_effects_can_use_dep_values() {
    let app = AppBuilder::new()
        .plugin(EffectfulEcho)
        .provide(Alpha(5))
        .build_state()
        .await;

    // `store_data` from build landed in plugin_data.
    assert_eq!(app.get_plugin_data::<StoredData>().map(|d| d.0), Some(5));

    // …and the layer build added produced a reachable route.
    let router = app.build();
    let (status, body) = get_route(router, "/late").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "late-5");
}
