#[test]
fn compile_fail() {
    let t = trybuild::TestCases::new();
    t.compile_fail("compile-fail/*.rs");
}
