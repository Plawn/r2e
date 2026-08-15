//! `GraphHandle` — the deferred-fill handle on the resolved graph — and
//! `Late<T>`, the underlying write-once cell (kept as an escape hatch).

use r2e_core::plugin::{GraphHandle, PluginBuildContext, PluginBuildError, PreStatePlugin};
use r2e_core::type_list::BeanAccess;
use r2e_core::{AppBuilder, Late};

use crate::fixtures::Alpha;

// ── GraphHandle ──────────────────────────────────────────────────────────────

/// A plugin-provided bean holding the `GraphHandle` its build captured, plus
/// what the handle looked like DURING build (must be empty — deps come
/// through `Deps`, the handle is for after-boot resolution).
#[derive(Clone)]
struct HandleHolder {
    handle: GraphHandle,
    empty_during_build: bool,
}

struct GraphCapturePlugin;

impl PreStatePlugin for GraphCapturePlugin {
    type Provided = (HandleHolder,);
    type Deps = ();
    type Config = ();

    async fn build(
        self,
        _deps: (),
        _config: Option<()>,
        ctx: &mut PluginBuildContext,
    ) -> Result<(HandleHolder,), PluginBuildError> {
        let handle = ctx.graph();
        Ok((HandleHolder {
            empty_during_build: handle.get().is_none(),
            handle,
        },))
    }
}

#[r2e_core::test]
async fn graph_handle_fills_after_build_state() {
    let app = AppBuilder::new()
        .plugin(GraphCapturePlugin)
        .provide(Alpha(21))
        .build_state()
        .await;

    let holder = app.state().get::<HandleHolder>();
    // During build the graph did not exist yet…
    assert!(holder.empty_during_build, "handle is empty during build");
    // …after build_state the same handle sees the full resolved graph.
    assert!(holder.handle.get().is_some(), "handle filled after resolve");
    assert_eq!(holder.handle.bean::<Alpha>(), Some(Alpha(21)));
    // Missing beans resolve to None, not a panic.
    assert_eq!(holder.handle.bean::<String>(), None);
}

#[test]
fn default_graph_handle_is_empty() {
    let handle = GraphHandle::default();
    assert!(handle.get().is_none());
    assert_eq!(handle.bean::<Alpha>(), None);
}

// ── Late<T> (escape hatch) ───────────────────────────────────────────────────

#[test]
fn empty_cell_reads_none() {
    let cell: Late<String> = Late::new();
    assert!(cell.get().is_none());
}

#[test]
fn fill_then_get() {
    let cell = Late::new();
    cell.fill(42u32).unwrap();
    assert_eq!(cell.get(), Some(&42));
}

#[test]
fn first_fill_wins() {
    let cell = Late::new();
    cell.fill("first").unwrap();
    assert_eq!(cell.fill("second"), Err("second"));
    assert_eq!(cell.get(), Some(&"first"));
}

#[test]
fn clones_share_storage() {
    // Clones are handed out BEFORE the fill, and a fill through any handle
    // must be visible to all of them.
    let shell: Late<u32> = Late::new();
    let handed_out = shell.clone();
    assert!(handed_out.get().is_none());

    shell.fill(7).unwrap();
    assert_eq!(handed_out.get(), Some(&7));

    // Clones taken after the fill see it too.
    assert_eq!(shell.clone().get(), Some(&7));
}

#[test]
fn expect_returns_filled_value() {
    let cell = Late::new();
    cell.fill("ready").unwrap();
    assert_eq!(*cell.expect("my value"), "ready");
}

#[test]
fn expect_on_empty_cell_panics_with_guidance() {
    let cell: Late<u32> = Late::new();
    let err = std::panic::catch_unwind(|| {
        cell.expect("grpc backend");
    })
    .unwrap_err();
    let msg = err
        .downcast_ref::<String>()
        .expect("panic payload should be a String");
    assert!(msg.contains("grpc backend"), "names the value: {msg}");
    assert!(msg.contains("u32"), "names the type: {msg}");
    assert!(
        msg.contains("before it was filled"),
        "points at the lifecycle: {msg}"
    );
}

#[test]
fn default_is_empty() {
    let cell: Late<u8> = Late::default();
    assert!(cell.get().is_none());
}

#[test]
fn debug_shows_fill_state() {
    let cell: Late<u32> = Late::new();
    assert_eq!(format!("{cell:?}"), "Late(<unfilled>)");
    cell.fill(3).unwrap();
    assert_eq!(format!("{cell:?}"), "Late(3)");
}
