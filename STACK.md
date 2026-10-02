# STACK — local jcode stack (generated 2026-09-30, do not hand-edit)

Source: `STACK.toml`. Regenerate: `./gen-stack-md.sh`.
Policy: build locally, publish once. No new PRs until stack declared done.
Exception: genuine fixes to upstream bugs may file anytime.

Stack HEAD: `62de8bb63` (`62de8bb6306d4b58b30f957349c7575d8f729899`)
Binary: `jcode v0.88.185-dev (6d83274ec)` — STALE: code changed since binary 6d83274ec — rebuild

## Branches

| Branch | State | Tip | PR | Note |
|--------|-------|-----|----|------|
| feat/compaction-hooks | in-stack | 6103dee85 | 1282 | Base of the stack. Garden docs + historian depend on it. |
| feat/tool-result-clearing | in-stack | 74265edcc | 1313 | Send-view clearing + offload. KV-invalidation record() ported to stack HEAD. |
| feat/pressure-notices | in-stack | 73b520175 | 1312 |  |
| feat/pre-request-transform | in-stack | ab0f4780d | 1280 |  |
| feat/recurring-schedules | in-stack | 09141fb63 | 1281 |  |
| feat/span-citations | in-stack | d2bf6fe38 | 1315 |  |
| feat/repomap-provider | in-stack | 754a98687 | 1320 |  |
| feat/memory-age-hedge | in-stack | 478eb1092 | - | UN-PR-D: file in publish batch. tunable-dedup-rrf rides nested inside. |
| feat/offload-with-ref | in-stack | a367dcb14 | - | UN-PR-D: file in publish batch. |
| feat/summary-schema | in-stack | 060a1e90a | - | UN-PR-D: file in publish batch. |
| feat/progress-file | in-stack | 61c15d332 | - | UN-PR-D: file in publish batch. fix/tui-agentsmd-working-dir rides nested inside. |
| feat/mcp-request-timeout-config | in-stack | 7701e0fae | - | UN-PR-D: file in publish batch. Was dropped by stale script once; manifest now prevents repeats. |
| feat/garden-historian-lite | in-stack | eaac2e23e | - | UN-PR-D, DO-NOT-FILE-EARLY: docs sample depends on #1282. #1515 filed prematurely, CLOSED. |
| feat/session-worker-view | in-stack | c0d5f64f2 | 1516 | Worker filter tab + run --parent + hook tagging. Stack carries it as cherry-pick a8f61ead4 (branch tip c0d5f64f2 has 29 upstream-behind commits); PR branch is 1 clean commit. Rebuild merges branch tip: expect upstream drift review. |
| exp/gr-recency | in-stack | 8898d8dd8 | - | G-R recency prior for hybrid RRF fusion (default-off gate, w_r=0.0 inert). Merged 2026-09-30 post-rebuild-script; rebuild script predates it — entry added so future rebuilds keep it. Sweep: a667 directional gain at w=0.05 per-category tau; blind confirm + non-zero defaults still pending (GR-SWEEP-VERDICT.md). |
| feat/tunable-dedup-rrf | nested | 373dfbd39 | 1314 | Rides inside feat/memory-age-hedge. |
| fix/tui-agentsmd-working-dir | nested | 61c15d332 | - | Rides inside feat/progress-file. Clean export fix/tui-agentsmd-clean carries PR #1514. |
| feat/rerank-fuse | parked | f64cda22a | - | OFF by design: upstream rejected local cross-encoders, #1228 open. |
| fix/tui-agentsmd-clean | export | 5e1cbd89d | 1514 | 5 commits over master, bot P1/P2 fixed, tests pass. OPEN. |
| feat/garden-historian-clean | export | b38236814 | - | 1 commit docs-only. #1515 CLOSED premature. Re-file only after #1282 lands. |

## Live verification

```
62de8bb63 Merge branch 'exp/gr-recency' into local/pr-stack
28e91ad0a fix(harness): seed scratch model cache from real cache (hermetic runs)
6d83274ec docs: regen STACK.md at 786bf6f77 (prefilter merge)
786bf6f77 merge: slot-B hybrid prefilter default-off + shadow plumbing + prefilter96 arms
0162c2d3b docs: regen STACK.md at 2dbce2443 (P2 tiebreak merge)
2dbce2443 merge: P2 deterministic (score desc, id asc) tiebreak F1-F10
baf038d1d feat(harness): C4 multihop assembly floor (r5>=0.80, r10>=0.90)
7342a2144 docs: regen STACK.md at ed9ee43d5 (jev-proxy+judge merge)
ed9ee43d5 merge: Jev proxy route + judge Tier-2 calibration (Phase 3 intel)
e8bd04e64 docs: regen STACK.md at a64c4d678 (wiring+R2+L1 merge)
a64c4d678 merge: write-path embedding + R2 anchor gate + L1 dense weight 3.0 (Phase 3 impl)
bd1dd6ea1 docs: regen STACK.md at 5753eb39b (stale-writer merge)
5753eb39b merge: temporal-validity writer R1-R4 + stale-competition arm (Phase 3 impl #2)
4876f62ac chore: gitignore harness run-ledger artifacts (keep committed green reference)
c151f78e8 fix(fork): resolve E0428 duplicate transform_test_config in hooks tests
ee706cb9e docs: regen STACK.md at 4f32199e1 (masking+harness merge, binary v0.88.162-dev)
4f32199e1 merge: tool-result-clearing masking tier-1 + acceptance harness (Phase 1)
ec4d0fa8b chore: rebuild-surviving tooling snapshot + worker-view cherry-pick note
4e2520e5e chore: code-aware binary freshness + Tip SHAs in gen-stack-md.sh
d025c48cb chore: STACK.md generated snapshot at e9895a47e
local/pr-stack not on this base
```
