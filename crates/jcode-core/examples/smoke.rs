//! Exercise the small utilities in `jcode_core` (the
//! dependency-light primitives crate).
//!
//! Demonstrates:
//! - `id::new_id` — generate a prefixed unique id
//! - `id::new_memorable_session_id` — generate a two-word memorable
//!   session id and the round-trip `extract_session_name` helper
//! - `util::truncate_str` — byte-safe truncation
//! - `util::estimate_tokens` — rough 4-chars-per-token estimator
//!
//! Run with: `cargo run -p jcode-core --example smoke`

use jcode_core::id;
use jcode_core::util;

fn main() {
    let prefixed = id::new_id("agent");
    println!("id::new_id(\"agent\") = {prefixed}");

    let (session_id, name) = id::new_memorable_session_id();
    println!("id::new_memorable_session_id() = {session_id}  (name = {name})");

    let recovered = id::extract_session_name(&session_id).unwrap_or("<none>");
    assert_eq!(recovered, name);
    println!("id::extract_session_name round-trip: {recovered}");

    let long = "a".repeat(120);
    let truncated = util::truncate_str(&long, 20);
    println!(
        "util::truncate_str(120 chars, 20) -> len {} ({:?})",
        truncated.len(),
        truncated
    );

    let sample = "the quick brown fox jumps over the lazy dog";
    let est = util::estimate_tokens(sample);
    println!("util::estimate_tokens(\"{sample}\") = {est} tokens (rough 4c/tok)");
}
