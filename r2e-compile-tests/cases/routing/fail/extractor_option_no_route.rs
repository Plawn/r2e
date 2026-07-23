//! An `Option<T>` request-scoped field where `T` has NO optional extraction
//! route — neither R2E's `OptionalFromRequestPartsVia` nor axum's
//! `OptionalFromRequestParts` — must fail with the guided
//! `on_unimplemented` diagnostics, not an opaque bound error. This pins
//! which message an app author actually sees (the outer
//! `Option<T>: FromRequestPartsVia` bound, whose notes point at both
//! optional traits), complementing the dual-route fixtures
//! (`extractor_dual_route_*.rs`) that pin the too-MANY-routes case.

use r2e::prelude::*;

/// Implements no extraction trait at all.
#[derive(Clone)]
pub struct Widget;

#[controller(path = "/w")]
pub struct WidgetController {
    #[inject(request)]
    widget: Option<Widget>,
}

#[routes]
impl WidgetController {
    #[get("/")]
    async fn show(&self) -> String {
        format!("{}", self.widget.is_some())
    }
}

fn main() {
    let _ = async {
        AppBuilder::new()
            .build_state()
            .await
            .register_controller::<WidgetController>()
    };
}
