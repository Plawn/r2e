//! `RouteInfo` says what the route really does (Tasker #1272).
//!
//! Two things the routes macro used to get wrong:
//!
//! - **Path parameters** were named after the handler's binding pattern, so
//!   `Path((ws, user)): Path<(WorkspaceId, UserId)>` published a single
//!   parameter called `"(ws, user)"`. They are now the placeholders of the
//!   route's full path template, typed positionally by the `Path<T>`
//!   extractor.
//! - **The success status** defaulted to 201 for `POST` and 204 for `DELETE`,
//!   while the generated handler forwards the return value untouched — a plain
//!   `Json<T>` answers 200 whatever the verb. A 204 also made the OpenAPI
//!   builder drop the response body. The default is now 200 for every verb;
//!   `#[status(N)]` declares anything else.

use r2e_core::controller::Controller;
use r2e_core::di::meta::{MetaRegistry, ParamLocation, RouteInfo};
use r2e_core::http::extract::Path;
use r2e_core::http::StatusCode;
use r2e_core::type_list::HNil;
use r2e_macros::{controller, routes, Params};
use serde::Deserialize;

/// A `Params` struct that declares one of the template's placeholders itself.
#[derive(Params, Deserialize)]
struct MemberParams {
    #[param(path)]
    user: u32,
    verbose: Option<bool>,
}

#[controller(path = "/meta")]
struct MetaController {}

#[routes]
impl MetaController {
    #[get("/pair/{ws}/{user}")]
    async fn pair(&self, Path((ws, user)): Path<(String, u64)>) -> String {
        format!("{ws}/{user}")
    }

    /// The binding name is irrelevant: `Path` is positional.
    #[get("/renamed/{id}")]
    async fn renamed(&self, Path(item_id): Path<i32>) -> String {
        item_id.to_string()
    }

    /// No destructuring pattern at all.
    #[get("/whole/{a}/{b}")]
    async fn whole(&self, path: Path<(bool, f64)>) -> String {
        format!("{:?}", path.0)
    }

    /// The extractor's arity does not match the template: nothing to zip.
    #[get("/untyped/{a}/{b}")]
    async fn untyped(&self, Path(only): Path<u8>) -> String {
        only.to_string()
    }

    /// No extractor reads the placeholder; it is still a parameter of the URL.
    #[get("/unread/{slug}")]
    async fn unread(&self) -> &'static str {
        "ok"
    }

    /// The `Params` field wins over the positional typing, and keeps the
    /// template's order.
    #[get("/members/{ws}/{user}")]
    async fn members(&self, params: MemberParams) -> String {
        format!("{} {:?}", params.user, params.verbose)
    }

    #[get("/verb")]
    async fn verb_get(&self) -> &'static str {
        "ok"
    }

    #[post("/verb")]
    async fn verb_post(&self) -> &'static str {
        "ok"
    }

    #[put("/verb")]
    async fn verb_put(&self) -> &'static str {
        "ok"
    }

    #[patch("/verb")]
    async fn verb_patch(&self) -> &'static str {
        "ok"
    }

    #[delete("/verb")]
    async fn verb_delete(&self) -> &'static str {
        "ok"
    }

    #[post("/created")]
    #[status(201)]
    async fn created(&self) -> (StatusCode, &'static str) {
        (StatusCode::CREATED, "ok")
    }

    #[delete("/gone")]
    #[status(204)]
    async fn gone(&self) -> StatusCode {
        StatusCode::NO_CONTENT
    }
}

/// Placeholders declared by the controller prefix belong to every route.
#[controller(path = "/tenants/{tenant}")]
struct PrefixedController {}

#[routes]
impl PrefixedController {
    #[get("/items/{id}")]
    async fn item(&self, Path((tenant, id)): Path<(String, u32)>) -> String {
        format!("{tenant}/{id}")
    }

    #[get("/items")]
    async fn items(&self, Path(tenant): Path<String>) -> String {
        tenant
    }
}

fn routes_of<C: Controller<HNil, W>, W>() -> Vec<RouteInfo> {
    let mut registry = MetaRegistry::new();
    C::register_meta(&mut registry);
    registry.take::<RouteInfo>()
}

fn route(routes: &[RouteInfo], method: &str, path: &str) -> RouteInfo {
    routes
        .iter()
        .find(|r| r.path == path && r.method.eq_ignore_ascii_case(method))
        .unwrap_or_else(|| panic!("no {method} {path}"))
        .clone()
}

/// `(name, location, type, required)` — the readable shape of a `ParamInfo`.
fn shape(route: &RouteInfo) -> Vec<(&str, ParamLocation, &str, bool)> {
    route
        .params
        .iter()
        .map(|p| {
            (
                p.name.as_str(),
                p.location,
                p.param_type.as_str(),
                p.required,
            )
        })
        .collect()
}

fn meta(path: &str) -> RouteInfo {
    route(&routes_of::<MetaController, _>(), "GET", path)
}

// ── Path parameters ─────────────────────────────────────────────────────────

#[test]
fn tuple_path_publishes_one_param_per_placeholder() {
    assert_eq!(
        shape(&meta("/meta/pair/{ws}/{user}")),
        vec![
            ("ws", ParamLocation::Path, "string", true),
            ("user", ParamLocation::Path, "integer", true),
        ]
    );
}

#[test]
fn path_param_is_named_after_the_template_not_the_binding() {
    assert_eq!(
        shape(&meta("/meta/renamed/{id}")),
        vec![("id", ParamLocation::Path, "integer", true)]
    );
}

#[test]
fn undestructured_path_extractor_is_typed_too() {
    assert_eq!(
        shape(&meta("/meta/whole/{a}/{b}")),
        vec![
            ("a", ParamLocation::Path, "boolean", true),
            ("b", ParamLocation::Path, "number", true),
        ]
    );
}

#[test]
fn arity_mismatch_falls_back_to_string() {
    assert_eq!(
        shape(&meta("/meta/untyped/{a}/{b}")),
        vec![
            ("a", ParamLocation::Path, "string", true),
            ("b", ParamLocation::Path, "string", true),
        ]
    );
}

#[test]
fn placeholder_without_extractor_is_still_published() {
    assert_eq!(
        shape(&meta("/meta/unread/{slug}")),
        vec![("slug", ParamLocation::Path, "string", true)]
    );
}

#[test]
fn params_struct_path_field_wins_and_keeps_template_order() {
    assert_eq!(
        shape(&meta("/meta/members/{ws}/{user}")),
        vec![
            ("ws", ParamLocation::Path, "string", true),
            ("user", ParamLocation::Path, "integer", true),
            ("verbose", ParamLocation::Query, "boolean", false),
        ]
    );
}

#[test]
fn controller_prefix_placeholders_are_published() {
    let routes = routes_of::<PrefixedController, _>();
    assert_eq!(
        shape(&route(&routes, "GET", "/tenants/{tenant}/items/{id}")),
        vec![
            ("tenant", ParamLocation::Path, "string", true),
            ("id", ParamLocation::Path, "integer", true),
        ]
    );
    assert_eq!(
        shape(&route(&routes, "GET", "/tenants/{tenant}/items")),
        vec![("tenant", ParamLocation::Path, "string", true)]
    );
}

// ── Success status ──────────────────────────────────────────────────────────

#[test]
fn default_status_is_200_for_every_verb() {
    let routes = routes_of::<MetaController, _>();
    for method in ["GET", "POST", "PUT", "PATCH", "DELETE"] {
        assert_eq!(
            route(&routes, method, "/meta/verb").response_status,
            200,
            "{method}"
        );
    }
}

#[test]
fn status_attribute_declares_another_status() {
    let routes = routes_of::<MetaController, _>();
    assert_eq!(route(&routes, "POST", "/meta/created").response_status, 201);
    assert_eq!(route(&routes, "DELETE", "/meta/gone").response_status, 204);
}
