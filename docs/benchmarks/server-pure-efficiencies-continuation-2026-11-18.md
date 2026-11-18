# Broader-workload efficiency continuation, November 18, 2026

The user requested continued work after the initial scoped plateau documented in `server-pure-efficiencies-2026-11-18.md`. Expanding beyond the original native Responses fixture found additional concrete ownership and repeated-work opportunities. The earlier plateau was not global exhaustion.

## Accepted changes

| Commit | Removed work | Measured evidence |
| --- | --- | --- |
| `781dc68ab` | Anthropic native repair deep-copied unchanged shared history. Replacement content is now allocated only after an actual removal, with borrowed membership IDs. | Two alternating optimized Linux thread-CPU/allocation probes improved all five cases. Normal-history repair was 49.7–84.7 times faster, malformed repair 1.047–1.061 times faster. |
| `3de375bd4` | Raw native replay constructed display summaries it discarded. Fallback decoded the same display twice. | Over 32,000 operations, native fallback CPU 33.736 to 18.144 ms, replay 34.986 to 22.010 ms. Large fallback 95.482 to 40.958 ms, replay 92.569 to 58.674 ms, downgrade 381.407 to 85.986 ms. Plain-text allocations unchanged. |
| `043f0a093` | ASCII raw search counted all matching terms after satisfying its threshold. | Many terms 83 to 31 ns CPU, large early hits 155,498 to 77,805 ns, late hits 233,888 to 156,158 ns. Allocations unchanged. |
| `5cbac9873`, `1ccfcb8f2` | Search projection copied plain text and tool outputs into intermediate strings and then joined them. It now borrows payloads, directly returning ordinary single-block messages without a vector. | Final 16-round alternating probe: small text 54 to 3 ns, many text blocks 3,306 to 1,183 ns. A 1.5 MB text dropped from 3,000,096 allocation-request bytes to zero, a 1.95 MB result from 3,900,096 to zero. All eleven shapes improved versus the original implementation. |
| `2a0bc18b0` | Hoisting or deduplicating one block detached shared content in unaffected messages. Lazy filtering now shares unchanged messages without extra payload passes. | Two alternating optimized thread-CPU/allocation probes passed all twelve cases. Function-local speedups ranged from 1.34 times for empty-heavy hoisting to 706 times for sparse large-history deduplication. |
| `d38eda7db` | Raw replay cloned call input JSON into a temporary lookup map although its source was already borrowed. The map now borrows raw inputs and owns synthesized inputs. | Versus accepted `3de375bd4`, median replay CPU fell 9.6% for native and 29.9% for large inputs. Large replay allocation-request bytes fell 46.1%. Fallback medians did not regress, plain-text allocations remained identical. |

These are function-local CPU and cumulative allocation-request measurements, not whole-request speedups or resident-memory reductions. Very large ratios reflect eliminating copies of megabyte payloads, not faster inference. Fixture construction is excluded. Frozen independent legacy implementations check exact content, separators, malformed inputs, empty-message rules, Unicode, ordering and same-ID cross-provider collisions. Storage-sharing tests check that unchanged content is actually shared.

## Reproduction and coverage

- `crates/jcode-provider-anthropic/src/repair_cpu_probe.rs`: native repair, frozen oracle and alternating Linux calling-thread CPU/allocation comparison.
- `crates/jcode-provider-anthropic/src/repair_cow_probe.rs`: hoist and dedupe sparse/dense/empty/prefix-tail cases.
- `crates/jcode-provider-anthropic/src/native_display_probe.rs`: original, first accepted and final native formatting implementations, replay/fallback/downgrade and ordinary text.
- `crates/jcode-app-core/examples/search_projection_probe.rs`: optimized projection benchmark, fixture setup excluded and result destruction included. Timing and allocation-counting passes are separate.
- `crates/jcode-session-types/tests/search_traversal_probe.md`: threshold traversal CPU table, exact-oracle coverage, commands and rejected experiments.

Use `justrust test -p jcode-provider-anthropic --release --lib probe:: -- --ignored --nocapture --test-threads=1` for the provider probes. Use `justrust run -p jcode-app-core --release --example search_projection_probe` for projection. Release here is the repository's optimized opt-level 1 profile, not the selfdev opt-level 0 profile.

## Rejected candidates

- Scoring with `match_indices` preserved results but repeated-hit scoring regressed about 10%. Counting only the suffix after the first hit helped late hits but regressed short exact and term-only scoring. Normalized string early-threshold experiments also regressed short/no-hit paths. All were restored. Only the measured ASCII raw fallback improvement was accepted.
- A generic iterator-chain single-projection fast path regressed the empty-separator multi-block case from 108 to 144 ns CPU. It was rejected. The narrower ordinary-single-block fast path passed all shapes and was accepted instead.
- Initial lazy hoisting regressed empty-message handling. Avoiding an unnecessary empty shared-content clone fixed it before acceptance.

## Integrated acceptance and residual ranking

Hosted integration on November 18 passed library/example Clippy across app-core, Anthropic, message-types and session-types, followed by **101 library tests** and **four optimized provider CPU/allocation probes**. Saved receipt: `20261118-015416216-1787053` for the optimized probes. The current and shared server were verified running `1ccfcb8f2`, containing all six accepted rows above.

The repeat three-way replay probe confirmed the followup versus its accepted baseline: native median batch CPU 1.399 to 1.245 ms, large 3.660 to 2.456 ms, Unicode 1.402 to 1.251 ms. Ordinary text remained allocation-identical. Across 32,000 large replay operations, original allocation-request traffic was 6,210,720,000 bytes versus 1,935,904,000 final bytes. The residual traffic includes the required owned API wire tree, not another whole-history repair copy. Large fallback retained 2,980,480,000 requested bytes across 32,000 operations for its actual rendered output and formatting. These cumulative numbers must not be interpreted as simultaneously resident memory.

## Explicit continuing gate

A renewed residual-owner inspection found another avoidable intermediate: full tool-call search projection serializes JSON to a temporary string and then copies it into the final formatted string. A direct final-buffer implementation is being measured against the already accepted projection, not merely the original baseline. This gate is therefore **not closed**. Fewer allocations alone do not qualify a candidate.

Remaining wire trees own their output by the existing API contract. Required changed-message survivors still need copies while source history stays shared. Removing these representations would require a separate transport/ownership redesign and validation, not an assumed free improvement. Cache eviction, history truncation and compression shrinking with extra copy cost are outside this no-tradeoff pass. The first-pass instrumented PSS increase remains unresolved, so no lower server RSS floor is claimed.
