//! `has_custom_fallback`: reading axum's private "default fallback" bit, the
//! thing R2E needs to install its framework 404 without stepping on a
//! fallback the app (or a plugin) installed.

use r2e_http::routing::{get, has_custom_fallback};
use r2e_http::Router;

fn with_routes<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new()
        .route("/a", get(|| async { "a" }))
        .route("/b/{id}", get(|| async { "b" }))
}

#[test]
fn a_fresh_router_has_the_default_fallback() {
    assert!(!has_custom_fallback(&Router::<()>::new()));
    assert!(!has_custom_fallback(&with_routes::<()>()));
}

#[test]
fn fallback_and_fallback_service_are_detected() {
    let r = with_routes::<()>().fallback(|| async { "nope" });
    assert!(has_custom_fallback(&r));

    let r = Router::<()>::new().fallback_service(get(|| async { "nope" }));
    assert!(has_custom_fallback(&r));
}

#[test]
fn the_bit_survives_state_layers_and_merges() {
    let custom = with_routes::<u8>().fallback(|| async { "nope" });
    let custom = custom.with_state(1u8);
    assert!(has_custom_fallback(&custom));

    let layered = custom.clone().layer(tower::layer::util::Identity::new());
    assert!(has_custom_fallback(&layered));

    // Merging a custom-fallback router into a default one carries it over.
    let merged = Router::<()>::new()
        .route("/z", get(|| async { "z" }))
        .merge(custom);
    assert!(has_custom_fallback(&merged));

    // Merging two default-fallback routers stays default.
    let merged = with_routes::<()>().merge(Router::new().route("/z", get(|| async { "z" })));
    assert!(!has_custom_fallback(&merged));
}

#[test]
fn a_nested_fallback_does_not_count_as_the_top_level_one() {
    // axum scopes a nested router's fallback under its prefix and leaves the
    // parent's default fallback in place — the framework 404 still has a slot.
    let nested = with_routes::<()>().fallback(|| async { "nested" });
    let r = Router::<()>::new().nest("/api", nested);
    assert!(!has_custom_fallback(&r));
}

#[test]
fn a_path_spelling_the_field_name_does_not_fool_the_check() {
    let r = Router::<()>::new().route(
        "/weird, default_fallback: false, catch_all_fallback: Default(Route)",
        get(|| async { "" }),
    );
    assert!(!has_custom_fallback(&r));
}
