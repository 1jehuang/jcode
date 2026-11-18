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

Commit `f726c82a4` introduces an explicitly sorted borrowed-field API using the existing canonical byte-range hashing loop. It avoids constructing an owned canonical JSON tree just for diagnostics. Explicit nulls, map order, item/system/tool hash ranges, lengths, and failure behavior retain frozen legacy parity. Runtime caller integration is tracked separately and must be accepted before claiming the production path benefits.

Two optimized alternating CPU probes passed all five shapes, receipts `20261118-044433509-2437540` and `20261118-044522387-2440087`. New/old thread-CPU ratios were 0.443–0.467 for empty payloads, 0.497–0.526 for tiny, 0.981 for large, 0.795–0.798 for nested, and 0.495–0.496 with all settings. Allocation requests fell from 21 calls/2,761 bytes to 6/628 for empty, and from 140/3,096,394 to 10/2,034,372 for large. Remaining fingerprint buffers still own serialized bytes required by this implementation.

CPU and allocation passes are separate. The thread-local allocation counter is disabled during CPU timing. Fixture setup is excluded, result destruction is included. These are function-local results, not inference latency or whole-server RSS claims.

## Current open acceptance gates

Owned OpenAI histories, full WebSocket replay construction, renderer image context, final rendered-buffer moves, canonical caller integration, and fresh WebSocket wire ownership are under independent optimized acceptance or implementation. Their early unoptimized measurements are not substituted for release-profile evidence. A fresh pinned profile, representative normal-runtime fixture, integrated tests, and coordinated activation remain required. Any new actionable ownership lead reopens implementation rather than being called a final plateau.
