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
| `d4547fbf2` | Tool-call search projection serialized JSON to an intermediate string and copied it again. It now writes directly into the final UTF-8 buffer, preserving null behavior. | Two alternating CPU/allocation probes improved all ten tool-input shapes against accepted `1ccfcb8f2`. Representative repeat CPU: null 59 to 35 ns, tiny object 109 to 64 ns, escaped 256 to 177 ns, nested 778 to 484 ns, large 1,804,601 to 1,173,517 ns. Frozen-oracle tests passed 3/3. An independent all-eleven-shape projection rerun also improved every case versus the original implementation. |

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
- Native summary direct `write!` and in-place trim preserved exact output and improved many workloads, but invalid-only inputs showed repeatable CPU regressions of 5.5% and 8.8%. Rejected and all production edits restored, despite fewer allocations elsewhere. Candidate/probe preserved in scratch for future investigation.
- Initial lazy hoisting regressed empty-message handling. Avoiding an unnecessary empty shared-content clone fixed it before acceptance.

## Integrated acceptance and residual ranking

Hosted integration on November 18 passed library/example Clippy across app-core, Anthropic, message-types and session-types, followed by **101 library tests**, **31 focused search tests** and **four optimized provider CPU/allocation probes**. Saved receipts: `20261118-015416216-1787053` for the optimized probes and `20261118-015618992-1795386` for search. The current and shared server were verified running `1ccfcb8f2`, containing all six accepted rows above.

The repeat three-way replay probe confirmed the followup versus its accepted baseline: native median batch CPU 1.399 to 1.245 ms, large 3.660 to 2.456 ms, Unicode 1.402 to 1.251 ms. Ordinary text remained allocation-identical. Across 32,000 large replay operations, original allocation-request traffic was 6,210,720,000 bytes versus 1,935,904,000 final bytes. The residual traffic includes the required owned API wire tree, not another whole-history repair copy. Large fallback retained 2,980,480,000 requested bytes across 32,000 operations for its actual rendered output and formatting. These cumulative numbers must not be interpreted as simultaneously resident memory.

## Final scoped gate

The renewed residual-owner review found two more intermediate-buffer candidates. Direct final-buffer tool JSON passed and was accepted as `d4547fbf2`. Direct native-summary formatting failed the CPU gate and was restored. No further **validated** free improvement remains from this investigated set. This is a scoped stopping point, not proof that all possible optimizations have been exhausted. Further whole-server heap work, other provider combinations and production-size transcript distributions remain broader investigations, particularly the unresolved instrumented PSS increase.

The final projection rerun passed all eleven shapes. Representative original-to-final CPU/allocation-request bytes: plain 1.5 MB text 735,051 to 18 ns and 3,000,096 to zero bytes, null call 61 to 29 ns and 147 to 113 bytes, full call 276 to 140 ns and 368 to 192 bytes. This controlled local optimized benchmark is separate from hosted correctness/lint acceptance. Hosted lint briefly encountered a sync-daemon connection closure, then succeeded on retry without any production workaround.

## Matched-instrumentation memory challenge

The initial retained run used `prof_accum:false`, while the subsequent cumulative run used `prof_accum:true`. Those resident-memory totals were not a controlled comparison. Both pinned first-pass binaries were rerun sequentially through the same cumulative fixture, with private home, test-session flag, unique socket, fake credentials and local-only native Responses output assertions. Artifacts: `~/.jcode/scratch/ram-fork/matched-accum-{before,after}-20261118/`.

At sixteen active sessions, private memory was **148.137 versus 156.289 MiB**, allocator live **75,341,160 versus 81,279,472 bytes**. After completion allocator live was **38,326,616 versus 44,488,696 bytes**, thread count **22 versus 34**. The previously observed approximately 54 MiB private-memory increase did not reproduce in magnitude, but a smaller increase remains. Thread population differs, while independently symbolized sampled retained application heap was **24.6 versus 24.5 MiB**. Thus the large payload-retention regression did not appear in this retained sample, but the roughly 6 MB allocator-live difference is not fully attributed. These binaries bracket the first pass, not the final broader-workload changes. This experiment does not establish a lower RSS floor or isolate the cause of the remaining increase.

Remaining wire trees own their output by the existing API contract. Required changed-message survivors still need copies while source history stays shared. Removing these representations would require a separate transport/ownership redesign and validation, not an assumed free improvement. Cache eviction, history truncation and compression shrinking with extra copy cost are outside this no-tradeoff pass. The first-pass instrumented PSS increase remains unresolved, so no lower server RSS floor is claimed.
