//! An FGA `from_path("...")` guard whose referenced path parameter is declared
//! in the route compiles. This covers two shapes:
//!   1. the parameter is a `{param}` in the method-level route path;
//!   2. the parameter is a `{param}` in the controller's `path = "..."` prefix
//!      (proving the compile-time check reads `PATH_PREFIX` and does not
//!      false-positive on prefix params).

use r2e::prelude::*;

// `FgaCheck` comes from `r2e_openfga::prelude`, re-exported by `r2e::prelude`
// under the `openfga` feature (included in `full`).

#[controller(path = "/documents")]
pub struct DocController;

#[routes]
impl DocController {
    // `doc_id` is declared in this method's own path.
    #[get("/{doc_id}")]
    #[guard(FgaCheck::relation("viewer").on("document").from_path("doc_id"))]
    async fn get_doc(&self, Path(_doc_id): Path<String>) -> Json<String> {
        Json(String::new())
    }
}

#[controller(path = "/orgs/{org_id}")]
pub struct OrgController;

#[routes]
impl OrgController {
    // `org_id` is declared on the controller PREFIX, not this method's path.
    #[get("/members")]
    #[guard(FgaCheck::relation("admin").on("organization").from_path("org_id"))]
    async fn members(&self) -> Json<String> {
        Json(String::new())
    }
}

fn main() {}
