#[test]
fn compile_fail() {
    let t = trybuild::TestCases::new();
    t.compile_fail("cases/*/fail/*.rs");
}

#[test]
fn compile_pass() {
    let t = trybuild::TestCases::new();
    t.pass("cases/*/pass/*.rs");
}
