//! `Routes::has_custom_fallback`: reading the backend's private "default
//! fallback" bit, the thing R2E needs to install its framework 404 without
//! stepping on a fallback the app (or a plugin) installed.

use r2e_http::routing::{get, Routes};

fn with_routes<S: Clone + Send + Sync + 'static>() -> Routes<S> {
    Routes::new()
        .route("/a", get(|_, _| async { "a" }))
        .route("/b/{id}", get(|_, _| async { "b" }))
}

#[test]
fn a_fresh_router_has_the_default_fallback() {
    assert!(!Routes::<()>::new().has_custom_fallback());
    assert!(!with_routes::<()>().has_custom_fallback());
}

#[test]
fn fallback_and_fallback_service_are_detected() {
    let r = with_routes::<()>().fallback(|_, _| async { "nope" });
    assert!(r.has_custom_fallback());

    let r = Routes::<()>::new().fallback_service(with_routes::<()>());
    assert!(r.has_custom_fallback());
}

#[test]
fn the_bit_survives_state_layers_and_merges() {
    let custom = with_routes::<u8>().fallback(|_, _| async { "nope" });
    let custom = custom.with_state(1u8);
    assert!(custom.has_custom_fallback());

    let layered = custom.clone().layer(tower::layer::util::Identity::new());
    assert!(layered.has_custom_fallback());

    // Merging a custom-fallback router into a default one carries it over.
    let merged = Routes::<()>::new()
        .route("/z", get(|_, _| async { "z" }))
        .merge(custom);
    assert!(merged.has_custom_fallback());

    // Merging two default-fallback routers stays default.
    let merged = with_routes::<()>().merge(Routes::new().route("/z", get(|_, _| async { "z" })));
    assert!(!merged.has_custom_fallback());
}

#[test]
fn a_nested_fallback_does_not_count_as_the_top_level_one() {
    // The backend scopes a nested tree's fallback under its prefix and leaves
    // the parent's default fallback in place — the framework 404 still has a
    // slot.
    let nested = with_routes::<()>().fallback(|_, _| async { "nested" });
    let r = Routes::<()>::new().nest("/api", nested);
    assert!(!r.has_custom_fallback());
}

#[test]
fn a_path_spelling_the_field_name_does_not_fool_the_check() {
    let r = Routes::<()>::new().route(
        "/weird, default_fallback: false, catch_all_fallback: Default(Route)",
        get(|_, _| async { "" }),
    );
    assert!(!r.has_custom_fallback());
}
