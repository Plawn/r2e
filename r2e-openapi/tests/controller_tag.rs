//! `#[controller(path = "…", tag = "…")]` — the OpenAPI tag is welded to the
//! declaration, not to the struct name.
//!
//! Splitting a fat controller in two (or renaming one) used to rewrite the
//! published spec, because the tag was the Rust struct name. With an explicit
//! `tag`, both halves keep publishing under the original group; without one,
//! the struct name remains the default.

use r2e_core::controller::Controller;
use r2e_core::di::meta::{MetaRegistry, RouteInfo};
use r2e_core::type_list::HNil;
use r2e_macros::{controller, routes};
use r2e_openapi::{build_spec, OpenApiConfig};

// Two controllers that used to be one `CatalogController`, split by concern
// but still one group in the published API.
#[controller(path = "/catalog/items", tag = "Catalog")]
pub struct CatalogItemsController {}

#[routes]
impl CatalogItemsController {
    #[get("/")]
    async fn list_items(&self) -> String {
        "items".to_string()
    }
}

#[controller(path = "/catalog/categories", tag = "Catalog")]
pub struct CatalogCategoriesController {}

#[routes]
impl CatalogCategoriesController {
    #[get("/")]
    async fn list_categories(&self) -> String {
        "categories".to_string()
    }
}

// No `tag` key: the struct name stays the tag, as before.
#[controller(path = "/health")]
pub struct HealthController {}

#[routes]
impl HealthController {
    #[get("/")]
    async fn health(&self) -> String {
        "ok".to_string()
    }
}

fn routes_meta() -> Vec<RouteInfo> {
    let mut registry = MetaRegistry::new();
    <CatalogItemsController as Controller<HNil, _>>::register_meta(&mut registry);
    <CatalogCategoriesController as Controller<HNil, _>>::register_meta(&mut registry);
    <HealthController as Controller<HNil, _>>::register_meta(&mut registry);
    registry.take::<RouteInfo>()
}

#[test]
fn explicit_tag_merges_two_controllers_into_one_group() {
    let spec = build_spec(&OpenApiConfig::new("Test API", "0.1.0"), &routes_meta());

    for path in ["/catalog/items/", "/catalog/categories/"] {
        let tags = spec["paths"][path]["get"]["tags"].as_array().unwrap();
        assert_eq!(
            tags.len(),
            1,
            "one tag per operation, got {tags:?} for {path}"
        );
        assert_eq!(tags[0], "Catalog", "path {path}");
    }
}

#[test]
fn without_a_tag_the_struct_name_is_still_the_tag() {
    let spec = build_spec(&OpenApiConfig::new("Test API", "0.1.0"), &routes_meta());
    let tags = spec["paths"]["/health/"]["get"]["tags"].as_array().unwrap();
    assert_eq!(tags[0], "HealthController");
}
