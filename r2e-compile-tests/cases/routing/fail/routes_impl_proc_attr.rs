//! An attribute below `#[routes]` that is not inert is a compile error.
//!
//! `#[routes]` replaces the annotated `impl` with TWO synthesized ones (routes
//! on the request façade, everything else on the controller core). Forwarding
//! an attribute to both is right for a lint allow or a doc comment and wrong
//! for an attribute macro: it would expand twice, duplicating whatever items it
//! emits. `#[cfg_attr(all(), ...)]` is not a way around it — the rule is stated
//! on the attribute that would be applied, so a non-inert payload is rejected
//! the same way (task #985).
use r2e::prelude::*;

#[derive(Clone)]
pub struct Svc;

#[controller(path = "/items")]
pub struct ItemController {
    #[inject]
    svc: Svc,
}

#[routes]
#[some_procedural_attribute]
impl ItemController {
    #[get("/")]
    async fn list(&self) -> String {
        String::new()
    }
}

fn main() {}
