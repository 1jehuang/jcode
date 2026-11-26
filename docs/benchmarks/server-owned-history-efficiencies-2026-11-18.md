# Owned-history efficiency continuation, November 18, 2026

This pass continues the explicit investigate, implement, measure, and reprofile loop. An earlier scoped plateau did not establish global exhaustion. Only exact-behavior changes that pass CPU and allocation gates are accepted. Production allocator/cache/concurrency tuning is excluded.

## Controlled allocator attribution

See `server-allocator-attribution-2026-11-18.md` and commit `dd8e007bf`. Reversed default run order changed the sign of the allocator difference. Diagnostic-only cache-disabled and controlled-worker runs did not reproduce a stable application-payload increase. These controls were not shipped, and do not establish a lower production RSS floor.

## Optimized native lookup acceptance

Commit `7e8ca3a2f` skips construction of unused display summaries when looking up native tool input. Frozen legacy parity covers malformed provider/type/status/ID data and duplicate order. The optimized repository release profile was independently rerun with:

```bash
justrust test --release -p jcode-message-types --test provider_native_lookup_probe -- --include-ignored --nocapture --test-threads=1
```

Receipt: `20261118-044907108-2471946`. Actual calling-thread CPU new/old ratios were 1.000 for empty lookup, 0.611 for tiny calls, 0.186 for tiny result hits, 0.065 for tiny misses, and approximately 0.015 for large result histories. Allocation requests never increased. Across 100 large no-hit operations, 630,000 allocation calls requesting 45,208,000 bytes became zero. These are cumulative allocation requests, not resident bytes.

## Borrowed canonical fingerprint fields

Commit `f726c82a4` introduces an explicitly sorted borrowed-field API using the existing canonical byte-range hashing loop. It avoids constructing an owned canonical JSON tree just for diagnostics. Explicit nulls, map order, item/system/tool hash ranges, lengths, and failure behavior retain frozen legacy parity. Commit `cc3dd247a` integrates the actual OpenAI caller, with frozen selected-field byte/hash/null/pointer tests.

Two optimized alternating CPU probes passed all five shapes, receipts `20261118-044433509-2437540` and `20261118-044522387-2440087`. New/old thread-CPU ratios were 0.443–0.467 for empty payloads, 0.497–0.526 for tiny, 0.981 for large, 0.795–0.798 for nested, and 0.495–0.496 with all settings. Allocation requests fell from 21 calls/2,761 bytes to 6/628 for empty, and from 140/3,096,394 to 10/2,034,372 for large. Remaining fingerprint buffers still own serialized bytes required by this implementation.

CPU and allocation passes are separate. The thread-local allocation counter is disabled during CPU timing. Fixture setup is excluded, result destruction is included. These are function-local results, not inference latency or whole-server RSS claims.

## Optimized owned-history and replay acceptance

Commits `20b4fe42a` and `4a07fc68e` transfer already-owned input into the final OpenAI request and skip discarded full-replay construction respectively. Two hosted optimized release runs, `20261118-045147905-2483302` and `20261118-045334143-2489085`, passed both alternating probes. Empty owned-history allocation remains identical. Nonempty owned request construction CPU fell 17–80%, with history-sized allocation requests eliminated. Full replay CPU ratios were approximately 0.26–0.43. Ordinary delta CPU stayed around baseline and allocation requests were exactly unchanged. See `openai-owned-history-2026-11-18.md` for complete tables, frozen oracle coverage, and caveats.

## Renderer acceptance

Commits `dbfe72162` and `84128b798` remove duplicate image-context tool-input ownership and move a sole final text/reasoning buffer into its rendered row. Hosted optimized acceptance in `171675d8a` supersedes preliminary debug timing. Both frozen parity tests and all eleven CPU/allocation shapes pass. Final-text large plain CPU ratio was 0.375 with requested bytes 525,661 to 263,517 per render. Mixed/reasoning cases establish nonregression, not speedup. Image-context fixtures include both accepted renderer changes and must not be treated as isolated final-text attribution. Complete results and hosted receipts are in `render-final-text-2026-11-18.md`.

## Fresh WebSocket ownership

Commit `b516c4b30` serializes a shallow sorted view of the fresh WebSocket envelope instead of deep-cloning the full request before replacing type and removing stream/background. Frozen original byte parity covers preexisting type values, Unicode/escapes, nested data, tool histories, images, explicit nulls and absent settings. The original retry request remains unchanged. Timeout helpers read only the unchanged reasoning effort. Scoped release CPU/allocation probes passed all five shapes. An independent hosted optimized repeat, `20261118-050541166-2532611`, measured CPU new/old ratios **0.736 empty, 0.496 tiny, 0.796 long history, 0.354 nested tools, and 0.930 native/image**. Allocation calls and requested bytes decreased in every shape. For 100 long-history operations, requested bytes fell from 544,016,000 to 421,620,600. This is cumulative allocation traffic, not retained memory or network latency.

## Fresh profile and residual gate

A hosted profiler build passed in `20261118-045934750-2508733`, after a transient hosted connection failure. The pinned executable SHA-256 is `3d312fee01b1ada46dab7863a26f16a042600068b4ecf7132a6652f881b605b1`. Its embedded version is unknown because this hosted build did not embed Git metadata. The build used production sources containing `b516c4b30`; subsequent `171675d8a` changes only the renderer evidence report. Identify the artifact by receipt and digest, not its embedded version.

Artifacts: `~/.jcode/scratch/ram-fork/owned-history-profile-20261118/`, with private home/socket, `JCODE_TEST_SESSION=1`, local fixture responses and no paid inference. Sixteen sessions each send eight exact 128 KiB tool outputs, with output bytes, call identities and order asserted. Fresh sampled application heap was **62.5 MiB active**, **24.5 MiB completed**, and **418.3 MiB cumulative allocations**. The earlier broader pass sampled about 62.1/24.4/445.5 MiB. These are sampled, not a controlled end-to-end memory reduction estimate. Background catalog refreshes and thread populations vary. Parent/child cumulative stack entries overlap and cannot be summed.

Residual owners reviewed:

- Restored transcript parsing retains approximately 16.1 MiB. These are the actual session messages, not a duplicate request history.
- Active API input retains approximately 16.6 MiB. The request builder itself now attributes about 0.1 MiB, rather than another history-sized tree.
- HTTP JSON body transport retains about 20.0 MiB active, with approximately 37.5 MiB cumulative allocation requests. The existing transport owns its serialized wire bytes. Streaming, compression policy changes and shrinking copies have separate CPU/behavior costs and were not silently substituted.
- History send attributes approximately 94.3 MiB cumulative, including about 78.0 MiB to protocol encoding. Earlier lifetime fixes already drop structured history before socket waits and wire bytes after sending. These are required output bytes, not all simultaneously retained.
- Canonical fingerprinting still requests about 37.5 MiB cumulatively for byte-range hashing buffers. Eliminating those buffers by streaming hashing had previously failed the CPU gate, so the rejected tradeoff was not reintroduced.
- Subscription metadata reads account for a visible transient input buffer. The startup stub already skips heavyweight vectors. Its source explicitly documents the slice-reader speed advantage over from_reader. Reusing metadata across lifecycle/freshness boundaries or changing parsing strategy is not an established free improvement.
- Pricing catalog parsing already borrows RawValue envelopes and retains only prices. Its approximately 1.7 MiB retained cache is not an unexamined full JSON tree.

No further identified, validated pure-efficiency candidate remains in these reviewed owners. This is a scoped plateau, not proof that every possible workload or future optimization is exhausted. A concrete new lead must reopen implementation and profiling.

## Normal-runtime and integrated acceptance

A coordinated normal build produced `d103e8a62`, without profiling instrumentation. A private exact-wire after/before/after sequence compared it with pinned `d521b8240`. Completed process CPU was **1.77 / 1.80 / 1.84 seconds**, active allocator live **63,353,120 / 63,364,496 / 63,197,728 bytes**, and active private memory **126.773 / 128.918 / 122.688 MiB**. Thread populations were 25/24/23. Whole-process CPU includes startup and background tasks. Earlier baseline CPU totals were larger, but that did not persist in the contemporaneous comparison. Therefore no whole-process CPU speedup or lower resident-memory floor is claimed. Exact request behavior passed all three runs.

Hosted integrated acceptance passed:

- Affected-package check: `20261118-045730254-2502671`.
- 247 OpenAI runtime tests: `20261118-045949410-2509587`.
- 210 provider-core tests: `20261118-050049070-2512499`.
- 11 focused renderer tests: `20261118-050056429-2513030`.
- Strict production-library Clippy: `20261118-050244311-2519443`.
- Strict provider-core/OpenAI-runtime/message-types test-inclusive Clippy: `20261118-050301637-2519442`.

Broad base test-inclusive Clippy additionally encountered **22 preexisting unrelated diagnostics**, including account-pool default reassignment, companion test dead fields, test module placement, and mobile test I/O handling. They were not suppressed or mixed into this optimization change. The renderer tests and production-library lint passed independently.

The current and shared daemon were activated on **`d103e8a62`** and independently verified with selfdev status and `server:info` at 13:05 UTC on November 18, 2026. This report-only followup does not change the tested production code.
