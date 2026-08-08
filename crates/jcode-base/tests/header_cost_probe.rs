// Measurement harness for the disk-backed work the TUI header performs on
// every rebuild. Run explicitly:
//
//   cargo test -p jcode-base --test header_cost_probe -- --nocapture --ignored
//
// This exists to keep the header cache's TTL-vs-signature tradeoff grounded in
// measured numbers on the target hardware rather than assumption.

#[test]
#[ignore = "measurement harness, not a pass/fail assertion"]
fn measure_auth_status_probe_cost() {
    use std::time::Instant;

    // Warm lazy statics and the page cache so we measure steady-state cost.
    let _ = jcode_base::auth::AuthStatus::check_fast();

    let mut samples: Vec<f64> = Vec::new();
    for _ in 0..25 {
        jcode_base::auth::AuthStatus::invalidate_cached_status();
        let started = Instant::now();
        let _ = jcode_base::auth::AuthStatus::check_fast();
        samples.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());

    println!(
        "AuthStatus::check_fast (cold cache) ms: p50={:.3} p90={:.3} max={:.3}",
        samples[samples.len() / 2],
        samples[samples.len() * 9 / 10],
        samples[samples.len() - 1],
    );

    // Cached reads are the common path; confirm they are effectively free.
    let mut cached: Vec<f64> = Vec::new();
    for _ in 0..25 {
        let started = Instant::now();
        let _ = jcode_base::auth::AuthStatus::check_fast();
        cached.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    cached.sort_by(|a, b| a.partial_cmp(b).unwrap());
    println!(
        "AuthStatus::check_fast (warm cache) ms: p50={:.4} max={:.4}",
        cached[cached.len() / 2],
        cached[cached.len() - 1],
    );
}

/// The TUI's prepared-header cache keys on the auth generation so a credential
/// change repaints the auth inventory on the next frame instead of waiting out
/// the header TTL. That only works if the real invalidation path bumps it.
///
/// This lives in `jcode-base` (its own test binary) rather than beside the
/// header tests because `invalidate_cached_status()` clears a process-global
/// cache that sibling tests in the `jcode-tui` binary share.
#[test]
fn invalidate_cached_status_bumps_the_auth_generation() {
    let before = jcode_base::auth::auth_status_generation();
    jcode_base::auth::AuthStatus::invalidate_cached_status();
    let after = jcode_base::auth::auth_status_generation();

    assert_ne!(
        before, after,
        "invalidate_cached_status() must bump the auth generation; the TUI \
         header cache relies on it to notice credential changes"
    );
}
