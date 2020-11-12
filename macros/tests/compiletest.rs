#[test]
fn gateway_config() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/gateway_config/*.rs");
}

#[ignore]
#[test]
/// These tests only pass on nightly, so ignore them by default.
///
/// Can be run with:
///
///     $ cargo +nightly test -- --ignored
///
fn gateway_config_nightly() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/gateway_config_nightly/*.rs");
}
