//! Bean-backed request extractors resolve from the application state `P`,
//! not the bean context — so a module's *private* provider cannot back one.
//! `#[inject]` of the same private bean is fine (context-resolved); the
//! extractor's `HasBean` bound must fail at `build_state()`.

use r2e::prelude::*;
use r2e::type_list::HasBean;
use r2e::{ViaBean, HttpError};

#[derive(Clone)]
pub struct Secret(pub &'static str);

#[bean]
impl Secret {
    fn new() -> Self {
        Self("s3cr3t")
    }
}

pub struct Stamp(pub String);

impl<S, I> FromRequestPartsVia<S, ViaBean<I>> for Stamp
where
    S: HasBean<Secret, I> + Send + Sync,
    I: Send + Sync,
{
    type Rejection = HttpError;

    async fn from_request_parts_via(
        _parts: &mut r2e::http::header::Parts,
        state: &S,
    ) -> Result<Self, Self::Rejection> {
        Ok(Stamp(state.get_bean().0.to_string()))
    }
}

#[controller(path = "/stamped")]
pub struct StampedController {
    #[inject(request)]
    stamp: Stamp,
}

#[routes]
impl StampedController {
    #[get("/")]
    async fn index(&self) -> String {
        self.stamp.0.clone()
    }
}

// Secret is provided but NOT exported — private to the module.
#[module(
    providers(Secret),
    controllers(StampedController),
    exports(),
    imports()
)]
pub struct StampModule;

fn main() {
    let _ = async {
        r2e::AppBuilder::new()
            .register_module::<StampModule>()
            .build_state()
            .await
    };
}
