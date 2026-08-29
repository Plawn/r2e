//! Item-level `#[cfg]` / `#[cfg_attr]` around `#[controller]` and `#[routes]`,
//! in both attribute orders (task #985).
//!
//! `#[controller]` and `#[routes]` rebuild the item they annotate, and neither
//! copies `#[cfg]` onto the items it generates — because rustc evaluates an
//! item-level `#[cfg]` BEFORE it invokes an attribute macro, whichever order the
//! two are written in, so a gated-out controller never reaches the macro and
//! leaves no dangling `ContextConstruct` / `Controller` impl behind. The
//! CHANGELOG states that as a rule; this module is what pins it instead of
//! assuming it.
//!
//! Every disabled case names types that do not exist: anything the macro left
//! behind fails this target with an unresolved-name error. The `#[bean]` /
//! `#[producer]` twins live in `r2e-core/tests/di/producer_attrs.rs`.

use r2e_core::prelude::*;
use std::sync::Arc;

// ── Disabled: `#[controller]`, both orders ─────────────────────────────────

#[controller(path = "/gone")]
#[cfg(any())]
struct CfgBelowController {
    #[inject]
    dep: ThisTypeDoesNotExist,
}

#[cfg(any())]
#[controller(path = "/gone")]
struct CfgAboveController {
    #[inject]
    dep: ThisTypeDoesNotExist,
}

// `#[cfg_attr]` expanding to a `#[cfg]` gates exactly the same way.
#[controller(path = "/gone")]
#[cfg_attr(all(), cfg(any()))]
struct CfgAttrController {
    #[inject]
    dep: ThisTypeDoesNotExist,
}

// ── Disabled: `#[routes]`, both orders ─────────────────────────────────────

#[routes]
#[cfg(any())]
impl ThisTypeDoesNotExist {
    #[get("/")]
    async fn list(&self) -> String {
        unreachable!()
    }
}

#[cfg(any())]
#[routes]
impl AlsoMissing {
    #[get("/")]
    async fn list(&self) -> String {
        unreachable!()
    }
}

// ── Enabled twins: `#[cfg]` must not *remove* what it lets through ─────────

#[derive(Clone)]
struct Label(Arc<str>);

#[controller(path = "/enabled")]
#[cfg(all())]
struct EnabledController {
    #[inject]
    label: Label,
}

#[routes]
#[cfg(all())]
impl EnabledController {
    #[get("/")]
    async fn list(&self) -> String {
        self.label.0.to_string()
    }
}

#[r2e_core::test]
async fn cfgd_in_controller_still_builds_from_the_graph() {
    let state = AppBuilder::new()
        .provide(Label(Arc::from("enabled")))
        .try_build_state()
        .await
        .expect("graph resolves");

    let core = <EnabledController as r2e_core::ContextConstruct>::from_context(state.bean_context());
    assert_eq!(core.label.0.as_ref(), "enabled");
}
