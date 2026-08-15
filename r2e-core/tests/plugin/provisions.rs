//! `Provided` tuples: arities 0–3, projections into the state HList, and the
//! test-pinning contract (pin one type vs pin them all).

use r2e_core::plugin::{PluginBuildContext, PluginBuildError, PreStatePlugin};
use r2e_core::type_list::BeanAccess;
use r2e_core::AppBuilder;

use crate::fixtures::{Alpha, Beta, BuildProbe, Gamma};

/// `Provided = ()` — a plugin that contributes no beans and exists purely for
/// its build-time effects. Its `build` must still run (the all-pinned skip is
/// guarded against the vacuously-true empty case).
struct NoProvider {
    probe: BuildProbe,
}

impl PreStatePlugin for NoProvider {
    type Provided = ();
    type Deps = ();
    type Config = ();

    async fn build(
        self,
        _deps: (),
        _config: Option<()>,
        _ctx: &mut PluginBuildContext,
    ) -> Result<(), PluginBuildError> {
        self.probe.mark();
        Ok(())
    }
}

struct SingleProvider;

impl PreStatePlugin for SingleProvider {
    type Provided = (Alpha,);
    type Deps = ();
    type Config = ();

    async fn build(
        self,
        _deps: (),
        _config: Option<()>,
        _ctx: &mut PluginBuildContext,
    ) -> Result<(Alpha,), PluginBuildError> {
        Ok((Alpha(7),))
    }
}

struct DualProvider {
    probe: BuildProbe,
}

impl PreStatePlugin for DualProvider {
    type Provided = (Alpha, Beta);
    type Deps = ();
    type Config = ();

    async fn build(
        self,
        _deps: (),
        _config: Option<()>,
        _ctx: &mut PluginBuildContext,
    ) -> Result<(Alpha, Beta), PluginBuildError> {
        self.probe.mark();
        Ok((Alpha(1), Beta("two".into())))
    }
}

struct TripleProvider;

impl PreStatePlugin for TripleProvider {
    type Provided = (Alpha, Beta, Gamma);
    type Deps = ();
    type Config = ();

    async fn build(
        self,
        _deps: (),
        _config: Option<()>,
        _ctx: &mut PluginBuildContext,
    ) -> Result<(Alpha, Beta, Gamma), PluginBuildError> {
        Ok((Alpha(3), Beta("three".into()), Gamma(true)))
    }
}

#[r2e_core::test]
async fn empty_provided_still_runs_build() {
    let probe = BuildProbe::default();
    let _app = AppBuilder::new()
        .plugin(NoProvider {
            probe: probe.clone(),
        })
        .build_state()
        .await;
    assert!(probe.ran(), "a Provided = () plugin must still build");
}

#[r2e_core::test]
async fn single_provision_lands_in_state_and_graph() {
    let app = AppBuilder::new().plugin(SingleProvider).build_state().await;
    // Projection is a normal member of the state HList…
    assert_eq!(app.state().get::<Alpha>(), Alpha(7));
    // …and of the resolved bean graph.
    assert_eq!(app.bean_context().as_ref().get::<Alpha>(), Alpha(7));
}

#[r2e_core::test]
async fn multi_provisions_project_every_element() {
    let app = AppBuilder::new()
        .plugin(DualProvider {
            probe: BuildProbe::default(),
        })
        .build_state()
        .await;
    assert_eq!(app.state().get::<Alpha>(), Alpha(1));
    assert_eq!(app.state().get::<Beta>(), Beta("two".into()));

    let app = AppBuilder::new().plugin(TripleProvider).build_state().await;
    assert_eq!(app.state().get::<Alpha>(), Alpha(3));
    assert_eq!(app.state().get::<Beta>(), Beta("three".into()));
    assert_eq!(app.state().get::<Gamma>(), Gamma(true));
}

#[r2e_core::test]
async fn pinning_one_type_wins_but_build_still_runs() {
    let probe = BuildProbe::default();
    let app = AppBuilder::new()
        // Pin only Alpha BEFORE install — the documented partial-pin contract:
        // the pinned type keeps its override, everything else comes from the
        // plugin, and `build` (side effects included) still runs.
        .override_bean(Alpha(99))
        .plugin(DualProvider {
            probe: probe.clone(),
        })
        .build_state()
        .await;

    assert_eq!(app.state().get::<Alpha>(), Alpha(99), "pin wins per type");
    assert_eq!(
        app.state().get::<Beta>(),
        Beta("two".into()),
        "unpinned element comes from the plugin"
    );
    assert!(probe.ran(), "a partial pin still runs build");
}

#[r2e_core::test]
async fn pinning_every_type_skips_build_entirely() {
    let probe = BuildProbe::default();
    let app = AppBuilder::new()
        // Whole-plugin mock: every Provided type pinned before install.
        .override_bean(Alpha(100))
        .override_bean(Beta("mock".into()))
        .plugin(DualProvider {
            probe: probe.clone(),
        })
        .build_state()
        .await;

    assert_eq!(app.state().get::<Alpha>(), Alpha(100));
    assert_eq!(app.state().get::<Beta>(), Beta("mock".into()));
    assert!(!probe.ran(), "all-pinned plugin never builds");
}
