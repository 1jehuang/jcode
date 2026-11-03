# [PERF-001] Heavy `.clone()` usage; audit hot paths for unnecessary copies

**ID:** PERF-001
**Category:** optimization
**Severity:** medium
**File:** repository-wide (crates/kcode-*)
**Status:** open

## Description
The workspace contains ~7,556 `.clone()` call sites. While many are legitimate (Arc/handle clones),
the density makes it likely that hot paths (per-token streaming, per-frame TUI render, per-request
message handling) clone large owned values (`String`, `Vec`, message structs) where a borrow would
do. This is a latent throughput/allocator-pressure issue rather than a confirmed regression.

## Evidence
```
grep -rIn "\.clone()" --include="*.rs" crates src | wc -l   # -> 7556
```

## Suggested fix
Profile before changing: run `cargo bench` / a streaming benchmark and use `perf`/`cargo-flamegraph`
on the TUI render loop and the streaming message path. Target the top allocators, replacing
`x.clone()` with `&x` or `Arc` clones where the value is read-only. Do not mass-refactor on count
alone.

## Effort
- L (half day)
