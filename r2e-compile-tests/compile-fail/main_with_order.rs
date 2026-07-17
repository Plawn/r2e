//! `order` is a test-only sequential-barrier argument. It has no meaning on
//! `#[r2e::main]` and must be a compile error spanned on the `order` literal.

#[r2e::main(order = 1)]
async fn run() {}

fn main() {}
