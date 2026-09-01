#[test]
fn type_bound_contracts() {
    // Rust diagnostics are not stable across toolchains. CI runs this suite
    // explicitly on the crate's pinned MSRV, while the behavioral matrix
    // skips snapshot comparison on stable, beta, and nightly.
    if std::env::var_os("ASYNC_RUNTIME_RUN_UI").is_none() {
        return;
    }

    let cases = trybuild::TestCases::new();
    cases.compile_fail("tests/ui/general_rejects_non_send_future.rs");
    cases.compile_fail("tests/ui/general_rejects_non_send_result.rs");
    cases.compile_fail("tests/ui/local_spawner_rejects_non_send_future.rs");
    cases.compile_fail("tests/ui/local_spawner_rejects_non_send_result.rs");
    cases.compile_fail("tests/ui/local_dispatch_rejects_non_send_closure.rs");
    cases.compile_fail("tests/ui/local_dispatch_future_rejects_non_send_future.rs");
    cases.compile_fail("tests/ui/local_domain_is_not_send.rs");
    cases.compile_fail("tests/ui/local_domain_is_not_sync.rs");
    cases.pass("tests/ui/local_spawn_local_accepts_non_send.rs");
    cases.pass("tests/ui/local_spawner_is_send_sync.rs");
}
