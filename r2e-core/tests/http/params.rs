//! `#[derive(Params)]` ↔ `Query<T>` parity.
//!
//! The migration promise (Tasker #1002) is that a DTO written for
//! `Query<T>` — serde renames and all — becomes a `Params` struct by
//! swapping the derive, with no attribute rewriting: the derive reads the
//! `#[serde(rename_all)]` / `#[serde(rename)]` / `#[serde(default)]` /
//! `#[serde(skip)]` the struct already carries, and a field with no r2e
//! attribute at all is a query parameter. The 400 body shape is an
//! app-level setting, pinned here in both forms.

use r2e_core::http::routing::get;
use r2e_core::http::{Router, StatusCode};
use r2e_core::web::params::{
    params_rejection_format, set_params_rejection_format, ParamsRejectionFormat,
};
use r2e_macros::Params;
use serde::Deserialize;
use std::sync::Mutex;

use crate::support::send_get;

// ── The real-world shape: a query DTO with serde renames ──────────────────

/// Modeled on the consumer-app structs that motivated the finding: a
/// `camelCase` wire contract over `snake_case` Rust fields, one field with
/// its own `rename`, optional filters, and defaults. Nothing here is an r2e
/// attribute — this compiled as `Query<SearchQuery>` unchanged.
#[derive(Params, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchQuery {
    /// Renamed by `rename_all` → `pageSize`.
    page_size: u32,
    /// Per-field rename wins over `rename_all` → `q`.
    #[serde(rename = "q")]
    search_term: String,
    /// The per-direction spelling serde also accepts.
    #[serde(rename(deserialize = "orderBy", serialize = "order_by"))]
    sort_field: Option<String>,
    /// Absent → `Default::default()`, like serde.
    #[serde(default)]
    include_archived: bool,
    /// Absent → the named factory, like serde.
    #[serde(default = "default_limit")]
    max_results: u32,
    /// Never read from the request at all.
    #[serde(skip)]
    computed: Vec<String>,
}

fn default_limit() -> u32 {
    50
}

async fn search(q: SearchQuery) -> String {
    format!(
        "{}|{}|{}|{}|{}|{}",
        q.page_size,
        q.search_term,
        q.sort_field.unwrap_or_else(|| "-".into()),
        q.include_archived,
        q.max_results,
        q.computed.len()
    )
}

/// An explicit r2e name is the most specific spelling and outranks serde's.
#[derive(Params, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExplicitQuery {
    #[query(name = "tenant_id")]
    tenant_id: String,
    #[serde(rename = "ignored-by-header")]
    #[header("x-trace-id")]
    trace_id: Option<String>,
}

async fn explicit(q: ExplicitQuery) -> String {
    format!("{}|{}", q.tenant_id, q.trace_id.unwrap_or_else(|| "-".into()))
}

/// Path parameters take their name from the same rename pipeline.
#[derive(Params, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct PathQuery {
    #[param(path)]
    order_id: String,
    detail_level: Option<u8>,
}

async fn order(p: PathQuery) -> String {
    format!("{}|{}", p.order_id, p.detail_level.unwrap_or(0))
}

/// `#[serde(flatten)]` means what a bare `#[params]` means: the nested
/// struct's own keys are read from the same request. Without this, a
/// flattened field would be read as one query parameter named after the
/// field — a silently wrong contract.
#[derive(Params, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageParams {
    page_index: u32,
    #[serde(default)]
    page_size: u32,
}

#[derive(Params, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListQuery {
    #[serde(flatten)]
    page: PageParams,
    filter_by: Option<String>,
}

async fn list(q: ListQuery) -> String {
    format!(
        "{}|{}|{}",
        q.page.page_index,
        q.page.page_size,
        q.filter_by.unwrap_or_else(|| "-".into())
    )
}

fn router() -> Router {
    Router::new()
        .route("/search", get(search))
        .route("/explicit", get(explicit))
        .route("/list", get(list))
        .route("/orders/{order-id}", get(order))
}

#[r2e_core::test]
async fn rename_all_and_per_field_renames_drive_the_query_keys() {
    let (status, body) = send_get(
        router(),
        "/search?pageSize=25&q=rust&orderBy=name&includeArchived=true&maxResults=5",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "25|rust|name|true|5|0");
}

#[r2e_core::test]
async fn serde_defaults_fill_absent_parameters() {
    let (status, body) = send_get(router(), "/search?pageSize=1&q=x").await;
    assert_eq!(status, StatusCode::OK);
    // include_archived → false (`#[serde(default)]`),
    // max_results → 50 (`#[serde(default = "default_limit")]`),
    // computed → empty (`#[serde(skip)]`).
    assert_eq!(body, "1|x|-|false|50|0");
}

#[r2e_core::test]
async fn snake_case_keys_are_not_accepted_when_renamed() {
    // The rename is authoritative: the Rust ident is no longer a valid key.
    let (status, body) = send_get(router(), "/search?page_size=25&q=rust").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.contains("pageSize"), "body: {body}");
}

#[r2e_core::test]
async fn explicit_r2e_names_outrank_serde_renames() {
    let (status, body) = send_get(router(), "/explicit?tenant_id=acme").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "acme|-");
}

#[r2e_core::test]
async fn path_parameter_uses_the_renamed_key() {
    let (status, body) = send_get(router(), "/orders/o-42?detail-level=3").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "o-42|3");
}

#[r2e_core::test]
async fn serde_flatten_reads_the_nested_struct_keys() {
    let (status, body) = send_get(router(), "/list?pageIndex=2&pageSize=20&filterBy=open").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "2|20|open");

    // The nested struct's own serde attributes still apply.
    let (status, body) = send_get(router(), "/list?pageIndex=1").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "1|0|-");
}

// ── Rejection body format (app-level, `server.params-rejection-format`) ───

/// The format is a process-global installed by `build_state()`; these two
/// tests write it, so they must not overlap.
static FORMAT_LOCK: Mutex<()> = Mutex::new(());

#[r2e_core::test]
async fn json_rejection_is_the_default_body_format() {
    let _guard = FORMAT_LOCK.lock().unwrap();
    set_params_rejection_format(ParamsRejectionFormat::default());
    assert_eq!(params_rejection_format(), ParamsRejectionFormat::Json);

    let resp = crate::support::raw(
        router(),
        "GET",
        "/search?q=x",
        &[],
        r2e_core::http::Body::empty(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        resp.headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("application/json")
    );
    let body = crate::support::body_string(resp).await;
    let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(
        parsed["error"].as_str().unwrap().contains("pageSize"),
        "body: {body}"
    );
}

#[r2e_core::test]
async fn plain_text_rejection_matches_raw_query_compat() {
    let _guard = FORMAT_LOCK.lock().unwrap();
    set_params_rejection_format(ParamsRejectionFormat::PlainText);

    let (status, body) = send_get(router(), "/search?q=x").await;
    set_params_rejection_format(ParamsRejectionFormat::Json);

    assert_eq!(status, StatusCode::BAD_REQUEST);
    // The bare message, no JSON envelope.
    assert_eq!(body, "Missing query parameter 'pageSize'");
}
