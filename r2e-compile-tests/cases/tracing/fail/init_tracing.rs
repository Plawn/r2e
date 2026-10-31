//! `init_tracing()` was removed: `launch` / `TestApp::boot` capture boot logs
//! and `build_state()` installs the subscriber. The call fails naming the
//! migration page.

fn main() {
    r2e::init_tracing();
}
