//! Deterministic regressions for the two env/render AB-BA sites in #1201.
//! A successful parallel run alone cannot rule out a timing-dependent deadlock.

fn test_body<'a>(source: &'a str, name: &str) -> &'a str {
    source
        .split_once(&format!("fn {name}() {{"))
        .expect("regression target must exist")
        .1
        .split("#[test]")
        .next()
        .unwrap()
}

fn env_precedes_render(body: &str, env_call: &str) -> bool {
    let env = body.find(env_call).expect("env acquisition must exist");
    let render = body
        .find("let _render_lock = scroll_render_test_lock();")
        .expect("render guard must cover the test, not just app setup");
    env < render
}

#[test]
fn inline_images_persistence_locks_env_before_render() {
    let body = test_body(
        include_str!("../src/tui/app/tests/scroll_copy_02/part_02.rs"),
        "test_alt_shift_i_toggles_inline_images_and_persists",
    );
    assert!(
        env_precedes_render(body, "let _env_guard = crate::storage::lock_test_env();"),
        "inline image persistence must acquire env before render (#1201)"
    );
}

#[test]
fn smoothness_benchmark_locks_env_before_render() {
    let body = test_body(
        include_str!("../src/tui/app/tests/smoothness_benchmark.rs"),
        "smoothness_benchmark_simulated_streaming_turn_stays_within_budget",
    );
    assert!(
        env_precedes_render(body, "with_reasoning_current_home(|| {"),
        "benchmark must acquire render inside the env-taking home helper (#1201)"
    );
}

#[test]
fn ordering_check_rejects_both_original_inversions() {
    for env_call in [
        "let _env_guard = crate::storage::lock_test_env();",
        "with_reasoning_current_home(|| {",
    ] {
        let render_call = "let _render_lock = scroll_render_test_lock();";
        assert!(!env_precedes_render(
            &format!("{render_call}\n{env_call}"),
            env_call
        ));
        assert!(env_precedes_render(
            &format!("{env_call}\n{render_call}"),
            env_call
        ));
    }
}
