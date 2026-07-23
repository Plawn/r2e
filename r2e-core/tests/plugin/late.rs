//! Tests for `Late<T>` — the shareable write-once cell plugins use to finish
//! a provided bean after `build_state()`.

use r2e_core::Late;

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
    // The bean-graph contract: clones are handed out BEFORE the fill, and a
    // fill through any handle must be visible to all of them.
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
        msg.contains("build_state"),
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
