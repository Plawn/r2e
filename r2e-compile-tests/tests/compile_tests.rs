#[test]
fn compile_fail() {
    let t = trybuild::TestCases::new();
    t.compile_fail("compile-fail/*.rs");
}

#[test]
fn compile_pass() {
    let t = trybuild::TestCases::new();
    t.pass("compile-pass/*.rs");
}
