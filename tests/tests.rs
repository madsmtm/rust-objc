#![cfg(any(target_os = "macos", target_os = "ios"))]

#[test]
fn ui() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/*.rs");
}
