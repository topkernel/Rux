//! Integration test: the checked-in fixture must produce its known counts.
//! Same assertions as the `--selftest` CLI flag (shared logic in
//! src/fixture.rs — one source of truth).

#[test]
fn fixture_known_counts() {
    match unsafe_meter::fixture::check() {
        Ok(msg) => println!("{}", msg),
        Err(e) => panic!("fixture self-test failed: {}", e),
    }
}
