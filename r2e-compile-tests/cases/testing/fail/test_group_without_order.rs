//! `group` on `#[r2e::test]` only names the sequential barrier for tests that
//! also declare an `order`. Using `group` without `order` is meaningless and
//! must be a compile error spanned on the `group` literal.

#[r2e::test(group = "seq")]
async fn my_test() {}

fn main() {}
