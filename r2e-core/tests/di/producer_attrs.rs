//! `#[producer]` must forward the attributes the user wrote on the annotated
//! function onto the re-emitted function (task #985).
//!
//! The macro rebuilds the function from its pieces (vis + signature parts +
//! body) so it can strip `#[config]` from the parameters. That rebuild used to
//! drop `item_fn.attrs` wholesale, so `#[allow]`, `#[deprecated]`, `#[inline]`
//! and doc comments written on a producer silently did nothing.
//!
//! Doc-comment survival is asserted separately, in
//! `r2e-compile-tests/cases/beans/pass/producer_doc_attrs.rs`: `missing_docs`
//! only fires on items effectively public from the crate root, which a module
//! of an integration-test binary is not.
//!
//! Most cases below are *compile-time* assertions: a `deny` turns a dropped
//! attribute into a build failure of this test target.

use r2e_core::prelude::*;

// ── 1. `#[allow(...)]` reaches the emitted function ────────────────────────
//
// The module denies `unused_variables`; the producer body has one. Only the
// `#[allow(unused_variables)]` the user wrote on the function can silence it,
// and only if the macro forwards it onto the emitted function.

#[deny(unused_variables)]
mod allow_is_honoured {
    use r2e_core::prelude::*;

    #[derive(Clone)]
    pub struct Allowed(pub u8);

    #[allow(unused_variables)]
    #[producer]
    pub fn make_allowed() -> Allowed {
        let ignored = 1u8;
        Allowed(2)
    }
}

// ── 2. `#[cfg]` gates the producer, in either attribute order ──────────────
//
// `any()` is always false, so the whole producer must disappear — including the
// generated `Producer`/`Registrable` impls, which would otherwise be dangling.
// The signatures deliberately name types that do not exist: anything left
// behind fails this target with an unresolved-name error.
//
// This works because rustc evaluates item-level `#[cfg]` BEFORE it invokes an
// attribute macro, whichever order the two are written in — which is why
// `#[producer]` must NOT copy `#[cfg]` onto its generated items (it would never
// see one). Both orders are pinned here so that stays true.

#[producer]
#[cfg(any())]
fn cfgd_out_below(dep: ThisTypeDoesNotExist) -> AlsoMissing {
    unreachable!()
}

#[cfg(any())]
#[producer]
fn cfgd_out_above(dep: ThisTypeDoesNotExist) -> AlsoMissing {
    unreachable!()
}

// The always-true twin: `#[cfg]` must not *remove* a producer that is enabled.
#[derive(Clone)]
struct CfgEnabled(&'static str);

#[producer]
#[cfg(all())]
fn cfgd_in() -> CfgEnabled {
    CfgEnabled("enabled")
}

// ── 3. `#[deprecated]` reaches the function without poisoning the macro ────
//
// The generated `produce()` calls the function, so the macro allows
// `deprecated` on its own items: a deprecated producer warns at the user's call
// sites, never from inside generated code.

#[derive(Clone)]
struct Legacy(u8);

#[deprecated(note = "use the new one")]
#[producer]
fn legacy_producer() -> Legacy {
    Legacy(7)
}

// ── 4. A producer with many parameters is not a clippy failure ─────────────
//
// A producer takes one parameter per dependency, so `too_many_arguments` fires
// on perfectly idiomatic producers — and (before #985) could not be silenced,
// since the `#[allow]` never reached the emitted function. `#[producer]` now
// emits `#[allow(clippy::too_many_arguments)]` itself; the `deny` here is what
// `cargo clippy -p r2e-core --test di` checks.

#[deny(clippy::too_many_arguments)]
mod many_dependencies {
    use r2e_core::prelude::*;

    #[derive(Clone)]
    pub struct D1;
    #[derive(Clone)]
    pub struct D2;
    #[derive(Clone)]
    pub struct D3;
    #[derive(Clone)]
    pub struct D4;
    #[derive(Clone)]
    pub struct D5;
    #[derive(Clone)]
    pub struct D6;
    #[derive(Clone)]
    pub struct D7;
    #[derive(Clone)]
    pub struct D8;

    #[derive(Clone)]
    pub struct Wide;

    #[producer]
    pub fn wide_producer(_a: D1, _b: D2, _c: D3, _d: D4, _e: D5, _f: D6, _g: D7, _h: D8) -> Wide {
        Wide
    }
}

// ── 5. `#[deprecated]` / `#[must_use]` really reach the emitted function ───
//
// `#[expect(...)]` is the assertion: it FAILS (as `unfulfilled_lint_expectations`,
// denied below) when the lint it names does not fire. So if `#[producer]` were
// to drop the attribute again, these two functions stop compiling — unlike a
// plain `#[allow]`, which is silent either way. This is what the earlier
// `#[inline]` case cannot do.

#[deny(unfulfilled_lint_expectations)]
mod attributes_are_load_bearing {
    use r2e_core::prelude::*;

    #[derive(Clone)]
    pub struct Legacy(pub u8);

    #[deprecated(note = "use the new one")]
    #[producer]
    pub fn legacy_thing() -> Legacy {
        Legacy(1)
    }

    #[expect(deprecated)]
    pub fn call_legacy() -> Legacy {
        legacy_thing()
    }

    #[derive(Clone)]
    pub struct Receipt(pub u8);

    #[must_use]
    #[producer]
    pub fn receipt_slip() -> Receipt {
        Receipt(2)
    }

    #[expect(unused_must_use)]
    pub fn drop_receipt() {
        receipt_slip();
    }
}

// ── 6. `#[cfg_attr]` gating, in either attribute order ─────────────────────
//
// A `#[cfg_attr]` that expands to a `#[cfg]` gates the item exactly like a
// literal one, before the attribute macro runs, whichever order they are
// written in. Same proof as case 2: the signatures name types that do not
// exist, so anything left behind fails this target.

#[producer]
#[cfg_attr(all(), cfg(any()))]
fn cfg_attr_out_below(dep: ThisTypeDoesNotExist) -> AlsoMissing {
    unreachable!()
}

#[cfg_attr(all(), cfg(any()))]
#[producer]
fn cfg_attr_out_above(dep: ThisTypeDoesNotExist) -> AlsoMissing {
    unreachable!()
}

// ── 7. The same two orders on `#[bean]` ────────────────────────────────────
//
// The CHANGELOG claims the rule holds for every attribute macro that rebuilds
// its item, not just `#[producer]`. `#[bean]` is pinned here; `#[controller]`
// and `#[routes]` in `r2e-core/tests/controller/attrs.rs`.

#[bean]
#[cfg(any())]
impl ThisTypeDoesNotExist {
    fn new(dep: AlsoMissing) -> Self {
        unreachable!()
    }
}

#[cfg(any())]
#[bean]
impl AlsoMissing {
    fn new(dep: ThisTypeDoesNotExist) -> Self {
        unreachable!()
    }
}

// The always-true twin: a cfg'd-IN bean must still register normally.
#[derive(Clone)]
struct CfgEnabledBean(&'static str);

#[bean]
#[cfg(all())]
impl CfgEnabledBean {
    fn new() -> Self {
        Self("bean-enabled")
    }
}

// ── Runtime checks ─────────────────────────────────────────────────────────

#[r2e_core::test]
async fn cfgd_in_producer_still_resolves() {
    let state = AppBuilder::new()
        .register::<CfgdIn>()
        .try_build_state()
        .await
        .expect("graph resolves");

    assert_eq!(state.bean_context().get::<CfgEnabled>().0, "enabled");
}

#[r2e_core::test]
#[allow(deprecated)]
async fn deprecated_producer_still_resolves() {
    let state = AppBuilder::new()
        .register::<LegacyProducer>()
        .try_build_state()
        .await
        .expect("graph resolves");

    assert_eq!(state.bean_context().get::<Legacy>().0, 7);
}

#[r2e_core::test]
async fn allowed_producer_still_resolves() {
    let state = AppBuilder::new()
        .register::<allow_is_honoured::MakeAllowed>()
        .try_build_state()
        .await
        .expect("graph resolves");

    assert_eq!(
        state.bean_context().get::<allow_is_honoured::Allowed>().0,
        2
    );
}

#[r2e_core::test]
async fn wide_producer_still_resolves() {
    use many_dependencies::*;

    let state = AppBuilder::new()
        .provide(D1)
        .provide(D2)
        .provide(D3)
        .provide(D4)
        .provide(D5)
        .provide(D6)
        .provide(D7)
        .provide(D8)
        .register::<WideProducer>()
        .try_build_state()
        .await
        .expect("graph resolves");

    let _: Wide = state.bean_context().get::<Wide>();
}

#[r2e_core::test]
async fn cfgd_in_bean_still_resolves() {
    let state = AppBuilder::new()
        .register::<CfgEnabledBean>()
        .try_build_state()
        .await
        .expect("graph resolves");

    assert_eq!(
        state.bean_context().get::<CfgEnabledBean>().0,
        "bean-enabled"
    );
}

#[r2e_core::test]
async fn load_bearing_attribute_producers_still_resolve() {
    use attributes_are_load_bearing::*;

    let state = AppBuilder::new()
        .register::<LegacyThing>()
        .register::<ReceiptSlip>()
        .try_build_state()
        .await
        .expect("graph resolves");

    assert_eq!(state.bean_context().get::<Legacy>().0, 1);
    assert_eq!(state.bean_context().get::<Receipt>().0, 2);

    // Keep the two `#[expect(...)]` assertion sites live: they are the mutation
    // detectors, and an unused fn would only warn.
    assert_eq!(call_legacy().0, 1);
    drop_receipt();
}
