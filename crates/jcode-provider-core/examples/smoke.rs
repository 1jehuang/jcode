//! Smoke test the small helper APIs in `jcode_provider_core`.
//!
//! `jcode_provider_core` is the trait-and-selection layer that every
//! concrete `jcode-provider-<name>-runtime` crate implements. It also
//! exposes pure helpers that don't need a live HTTP client — useful
//! for unit tests and for callers that want to roll their own
//! transport:
//!
//! - `fingerprint::stable_hash_str` / `stable_hash_json` — stable
//!   hashes for prompt-cache break detection
//! - `retry_after::retry_delay` — exponential backoff with a
//!   server-supplied hint, identical to the production formula
//!
//! Run with: `cargo run -p jcode-provider-core --example smoke`

use jcode_provider_core::fingerprint::{stable_hash_json, stable_hash_str};
use jcode_provider_core::retry_after::retry_delay;
use serde_json::json;
use std::time::Duration;

fn main() {
    let h1 = stable_hash_str("hello world");
    let h2 = stable_hash_str("hello world");
    let h3 = stable_hash_str("goodbye world");
    assert_eq!(h1, h2);
    assert_ne!(h1, h3);
    println!("stable_hash_str h1 == h2: {h1} == {h2}");
    println!("stable_hash_str h3:        {h3}");

    let j = json!({ "model": "claude-3", "messages": ["hi"] });
    let hj = stable_hash_json(&j);
    println!("stable_hash_json: {hj}");

    println!();
    println!("retry_delay(attempt, base=200ms, no server hint):");
    for attempt in 0..6 {
        let d = retry_delay(attempt, 200, None);
        println!("  attempt {attempt} -> {} ms", d.as_millis());
    }

    println!();
    println!("retry_delay(attempt, base=200ms, server hint=5s):");
    for attempt in 0..6 {
        let d = retry_delay(attempt, 200, Some(Duration::from_secs(5)));
        println!(
            "  attempt {attempt} -> {} ms (server-hint-respecting)",
            d.as_millis()
        );
    }
}
