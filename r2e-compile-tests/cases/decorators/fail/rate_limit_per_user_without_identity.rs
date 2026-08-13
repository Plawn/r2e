//! A per-user rate limit keys its bucket on `identity.sub()`, so it can only
//! ever work with an authenticated identity. On a controller with no identity
//! (no struct-level `#[inject(identity)]`, no identity parameter) the bucket
//! could never be keyed — sharing one anonymous bucket would silently turn the
//! per-user budget into a global one. `RateLimit` declares
//! `DecoratorSpec::REQUIRES_IDENTITY = true`, so `#[routes]` rejects the
//! placement at compile time instead of 401-ing in production.

use r2e::prelude::*;
use r2e::r2e_rate_limit::RateLimit;

#[controller(path = "/things")]
pub struct ThingController;

#[routes]
impl ThingController {
    #[get("/")]
    #[guard(RateLimit::per_user(5, 60))]
    async fn list(&self) -> Json<&'static str> {
        Json("things")
    }
}

fn main() {}
