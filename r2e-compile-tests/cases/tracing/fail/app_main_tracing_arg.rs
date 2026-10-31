//! `#[r2e::main(tracing = false)]` was removed: `build_state()` installs the
//! subscriber from the `tracing:` section (`tracing.enabled: false` opts
//! out). The error names the migration page.

#[r2e::main(tracing = false)]
async fn main() {}
