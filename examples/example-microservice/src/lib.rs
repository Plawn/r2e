//! example-microservice library.
//!
//! Two independent R2E services in one crate. Each is declared as a canonical
//! [`App`](r2e::prelude::App) — `product::ProductApp` and `order::OrderApp` —
//! and launched by its own thin binary (`src/product/main.rs`,
//! `src/order/main.rs`) via `r2e::launch!`.

pub mod order;
pub mod product;
pub mod shared;
