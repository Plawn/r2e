//! Bounded label helpers shared by the telemetry crates
//! (`r2e-prometheus`, `r2e-observability`).

use http_body_util::BodyExt;
use r2e_http::extract::MatchedPath;
use r2e_http::header::Method;
use r2e_http::labels::{method_label, route_label, OTHER_METHOD_LABEL, UNMATCHED_PATH_LABEL};
use r2e_http::routing::get;
use r2e_http::{Body, Request, Router};
use tower::ServiceExt;

#[test]
fn standard_methods_map_to_themselves() {
    for method in [
        "GET", "HEAD", "POST", "PUT", "DELETE", "CONNECT", "OPTIONS", "TRACE", "PATCH",
    ] {
        let m = Method::from_bytes(method.as_bytes()).unwrap();
        assert_eq!(method_label(&m), method);
    }
}

#[test]
fn extension_methods_collapse_into_the_other_label() {
    for method in ["PURGE", "FOOBAR", "LOCK"] {
        let m = Method::from_bytes(method.as_bytes()).unwrap();
        assert_eq!(method_label(&m), OTHER_METHOD_LABEL);
    }
}

#[test]
fn no_matched_path_yields_the_sentinel() {
    assert_eq!(route_label(None), UNMATCHED_PATH_LABEL);
}

#[tokio::test]
async fn matched_path_yields_the_route_template() {
    let router = Router::new().route(
        "/users/{id}",
        get(|matched: MatchedPath| async move { route_label(Some(&matched)).to_owned() }),
    );

    let req = Request::builder()
        .uri("/users/7")
        .body(Body::empty())
        .unwrap();
    let res = router.oneshot(req).await.unwrap();
    let body = res.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&body[..], b"/users/{id}");
}
