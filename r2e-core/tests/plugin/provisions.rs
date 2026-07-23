//! Tuple `Provided` on `PreStatePlugin`: beans a plugin contributes to the graph.

use r2e_core::plugin::{DeferredAction, PluginInstallContext, PreStatePlugin};
use r2e_core::type_list::BeanAccess;
use r2e_core::AppBuilder;

use crate::fixtures::{Alpha, Beta};

// ── Tuple `Provided` (PreStatePlugin) ──────────────────────────────────────

/// Provides a single bean via the one-tuple `(T,)` form.
struct SingleProvider;

impl PreStatePlugin for SingleProvider {
    type Provided = (Alpha,);
    type Deps = ();
    type Config = ();

    fn install(&mut self, _ctx: &mut PluginInstallContext<'_>) -> (Alpha,) {
        (Alpha(7),)
    }
}

/// Provides two beans in one plugin — the case that used to require
/// `RawPreStatePlugin`.
struct MultiProvider;

impl PreStatePlugin for MultiProvider {
    type Provided = (Alpha, Beta);
    type Deps = ();
    type Config = ();

    fn install(&mut self, _ctx: &mut PluginInstallContext<'_>) -> (Alpha, Beta) {
        (Alpha(42), Beta("hello".into()))
    }
}

/// Provides nothing — only registers a deferred action.
struct NoProvider;

impl PreStatePlugin for NoProvider {
    type Provided = ();
    type Deps = ();
    type Config = ();

    fn install(&mut self, ctx: &mut PluginInstallContext<'_>) {
        ctx.add_deferred(DeferredAction::new("no-provider", |_dctx| {}));
    }
}

#[r2e_core::test]
async fn zero_provision_plugin_builds_and_keeps_other_beans() {
    // `type Provided = ()` maps to TNil: nothing is added to the state, and
    // the builder still accepts the plugin (and its deferred action).
    let app = AppBuilder::new()
        .plugin(NoProvider)
        .provide(Alpha(1))
        .build_state()
        .await;
    assert_eq!(app.state().get::<Alpha>(), Alpha(1));
}

#[r2e_core::test]
async fn single_provision_plugin_resolves_from_state() {
    let app = AppBuilder::new().plugin(SingleProvider).build_state().await;
    let state = app.state();
    assert_eq!(state.get::<Alpha>(), Alpha(7));
    // Also resolvable through the retained bean context (the `#[inject]` path).
    assert_eq!(app.bean_context().as_ref().get::<Alpha>(), Alpha(7));
}

#[r2e_core::test]
async fn multi_provision_plugin_resolves_both_beans_from_state() {
    let app = AppBuilder::new().plugin(MultiProvider).build_state().await;
    let state = app.state();
    assert_eq!(state.get::<Alpha>(), Alpha(42));
    assert_eq!(state.get::<Beta>(), Beta("hello".into()));
    // Both are injectable via the bean context, by type.
    assert_eq!(app.bean_context().as_ref().get::<Alpha>(), Alpha(42));
    assert_eq!(
        app.bean_context().as_ref().get::<Beta>(),
        Beta("hello".into())
    );
}
