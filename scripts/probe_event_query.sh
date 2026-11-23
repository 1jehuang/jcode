#!/usr/bin/env bash
# Isolated hosted release comparison with a frozen age for exact byte parity.
set -euo pipefail
cd "$(dirname "$0")/.."
d=$(mktemp -d "${JCODE_SCRATCH_DIR:-${TMPDIR:-/tmp}}/event-query.XXXXXX")
trap 'rm -rf "$d"' EXIT
mkdir "$d/src"
cat > "$d/Cargo.toml" <<'TOML'
[package]
name="event-query-probe"
version="0.1.0"
edition="2021"
[dependencies]
serde={version="1", features=["derive"]}
serde_json="=1.0.149"
libc="0.2"
[features]
count=[]
TOML
src=crates/jcode-app-core/src/server
{
  echo 'use serde::{Serialize,Deserialize}; use std::{time::{Instant,SystemTime},collections::VecDeque,hint::black_box};'
  sed -n '/^\/\/\/ Event types for real-time event subscription/,/^\/\/\/ Ring buffer for recent swarm events/{ /^\/\/\/ Ring buffer for recent swarm events/!p; }' "$src/state.rs"
  sed -n '/^fn pretty_events</,/^}/p; /^fn event_payload(/,/^}/p' "$src/debug_events.rs" | sed 's/event.timestamp.elapsed().as_secs()/42u64/'
  cat scripts/probes/event_query.rs
} > "$d/src/lib.rs"
cd "$d"
justrust test --remote --release --lib -- --nocapture
justrust test --remote --release --lib --features count -- --nocapture
