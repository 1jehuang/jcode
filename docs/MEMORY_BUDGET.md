# Memory Regression Budget

Status: active guardrail
Updated: 2026-11-16

This document defines the current memory regression budget for jcode.

For live server triage and cause-specific remediation, see
[Jcode Server Memory Incident Runbook](./MEMORY_INCIDENT_RUNBOOK.md).

For idle-session cache reclamation, its configuration, measured RAM indicator,
and retained-state boundaries, see [Session Hibernation](./SESSION_HIBERNATION.md).

The goal is not to freeze memory usage forever. The goal is to make memory changes:
- measurable
- reviewable
- intentionally justified

Where possible, budgets below are tied to counters and caps already exposed by the codebase rather than guessed RSS numbers.

## How to collect the metrics

Use existing debug surfaces instead of ad hoc instrumentation:

- TUI aggregate memory profile: `:debug memory`
- TUI memory sample history: `:debug memory-history`
- Markdown cache profile: `:debug markdown:memory`
- Mermaid cache profile: `:debug mermaid:memory`
- Agent/session memory profile via debug socket: `agent:memory`
- Fast server incident classification: `server:memory-incident`
- Full server attribution: `server:memory`
- Process-lifetime timeline: `python scripts/analyze_runtime_memory_log.py --days 1`

Primary sources in code:
- `src/tui/app/debug_cmds.rs`
- `src/tui/memory_profile.rs`
- `src/session.rs`
- `src/tui/markdown.rs`
- `src/tui/mermaid.rs`
- `src/runtime_memory_log.rs`

## Budget model

We use two kinds of budgets:

1. Hard caps
- These are explicit limits already enforced by caches.
- Regressions here mean the code changed its bound or bypassed it.

2. Ratchet expectations
- These are expected relationships between memory counters.
- Regressions here are allowed only with explanation and updated docs/tests.

## Efficiency investigations and stopping criteria

Prefer eliminating redundant ownership over reducing useful retained state.
Preserve conversation history, exact persistence/replay, snapshot isolation,
rendered output, responsiveness, and cache utility. Cache eviction, compression
with extra decoding work, and delayed delivery are tradeoffs, not automatic wins.

For an optimization round:

1. Rank attributed owners before editing, but do not use a minimum size cutoff
   to dismiss identifiable redundant ownership. Pursue small obvious waste as
   well as material owners when exact behavior and acceptable CPU/latency can
   be preserved. An unattributed heap gap is not itself evidence of waste.
2. Pin and identify both binaries. Repeat the same workload against isolated
   sockets and homes, with settled samples at equal session/turn counts. Do not
   infer savings from restarting the production server.
3. Distinguish live allocator bytes, process PSS/private pages, unique backing
   allocations, string capacities, and serialized JSON sizes. JSON size is an
   attribution proxy, not a guaranteed heap bound. Exclude busy profiles explicitly.
4. Test exact equality, legacy serialization, replay, and mutation isolation.
   Verify the installed binary and live diagnostic schema after activation.
5. Continue automatically through concrete safe candidates, including small
   ones, without requiring repeated user prompts. Two ranked follow-up passes
   finding no further actionable candidate establish a practical stopping point,
   not a theorem that no optimization can exist. Record each remaining owner's
   disposition and any missing attribution or behavior/latency tradeoff. A wider
   redesign alone is not evidence that an opportunity is impossible. Do not
   reopen an unchanged inventory merely because theoretical potential exists.

### Authentication request ownership acceptance (2026-11-16)

MCP dynamic registration serializes borrowed redirect URI and stack-owned
metadata arrays directly instead of building an owned JSON map. Exact legacy
wire bytes are preserved for empty, ordinary, escaped, and Unicode inputs.
Token requests release their owned form fields immediately after serialization,
rather than retaining duplicate secret-bearing strings through response waits.
A builder regression verifies exact URL-encoded bytes after source deallocation.
The full hosted serial MCP OAuth suite passed all 22 tests, including existing
loopback login, refresh, and security coverage. These are structural allocation
and lifetime improvements, not measured production RSS reductions.

Antigravity project lookup retains stack-owned metadata structs with borrowed
strings instead of an owned nested JSON tree. Serialization key order and
endpoint fallback behavior are unchanged. All 11 hosted serial authentication
tests passed, including exact legacy bytes and builder ownership after the
metadata inputs leave scope. Per-attempt wire serialization is unchanged.

Gemini authorization-code exchange drops its owned form and copied credential
strings immediately after eager form encoding. A credential-free builder test
verifies exact URL-encoded bytes, trimming, optional verifier, and endpoint after
all source strings leave scope. All 12 hosted serial Gemini authentication tests
passed. No external OAuth calls or production RSS comparisons were used.

Device authorization metadata uses typed static references instead of an owned
JSON map. Exact legacy bytes and optional requested-tier omission are checked
for every tier. The full hosted subscription API group passed all 12 tests,
including builder-owned bytes after metadata leaves scope. No live login,
billing, payment, or plan mutation was performed.

OpenAI device-flow start and poll requests now serialize borrowed client/device
metadata directly instead of allocating temporary JSON maps and copied strings.
All 94 hosted serial OAuth tests passed. Added exact-byte and source-release
coverage includes empty fields, Unicode, escaping, and control characters.
Polling cadence, endpoints, timeouts, and error handling are unchanged. No real
OAuth requests were sent by the new tests, and no RSS saving is inferred.

### Report request ownership acceptance (2026-11-16)

Sponsor usage flushing moves its reports into a typed serialization view instead
of constructing a second nested JSON tree, then drops the source reports before
network response waits. Exact sorted legacy keys, report order, timeout, and
best-effort delivery policy are unchanged. All 5 scoped hosted tests passed,
including a source-drop marker checked before any send. No telemetry was sent.

Mobile command completion serializes borrowed session IDs or a short-lived
formatted error directly, without an intermediate owned JSON map. Exact legacy
success/error bytes, authorization, endpoint, and timeout remain unchanged. All
12 hosted mobile runner tests passed, including Unicode, escaping, empty IDs,
and chained-error wire coverage. No real commands were dispatched. These gains
are structural reductions in duplicate allocation and source lifetime only.

Mobile claim polling also borrows hostname metadata into direct serialization,
dropping the converted environment value before network waits. Empty and
Unicode/escaped hostnames preserve exact legacy bytes, endpoint, authorization,
and timeout. The hosted mobile runner group passed 46 tests with 1 ignored.
These include overlapping completion/local-runner coverage, not 46 additional
independent tests to add to earlier counts. No claim was sent to production.

Subscription device-token polling, email-code start, and email-code verification
serialize borrowed fields rather than allocating intermediate JSON maps. Exact
legacy bytes preserve which fields are trimmed and which are not, including
Unicode whitespace, empty strings, escaping, and control characters. All 14
hosted serial subscription API tests passed for this followup, including the
earlier device-metadata tests. No real login or email was initiated.

Scalar account request metadata now serializes stack-owned numeric fields
directly instead of constructing JSON maps. Exact bytes for zero, one, and
`u64::MAX` are checked using builders only. All 15 hosted subscription API tests
passed after this followup. No financial API was invoked, and validation,
authorization, endpoint, timeout, response handling, and billing policy did not
change. These tests overlap the earlier 14-test group.

Claude weekly/earned reset request metadata uses typed serialization instead of
temporary JSON maps and copied grant/request IDs. Pure builder tests verify exact
legacy bytes after input release. The existing local-fixture safety suites also
passed: 9 weekly-reset tests and 8 earned-reset tests. Confirmation, eligibility,
locking, durable pending markers, journal writes, idempotency, invalidation, and
non-refundable operation sequencing are untouched. No real reset request was
made and no grant was consumed. This is source-allocation reduction only.

### Catalog persistence ownership acceptance (2026-11-16)

Antigravity, Gemini, Copilot, Cursor, and Bedrock persistence serialize borrowed
catalog views instead of cloning model strings and collections solely to write
JSON. Antigravity's project request also borrows its optional project string.
Cache paths, endpoint/region scope, freshness rules, refresh behavior, and model
discovery remain unchanged. Exact compact and pretty legacy JSON tests include
arbitrary future models absent from fallback lists, Unicode, escaping, optional
fields, and empty collections. Bedrock additionally checks 64 combinations of
model/profile/route/set population and region, preserving its old set iteration
sequence without temporary cloned vectors.

Hosted validation passed: the full Gemini and Cursor runtime groups (98 tests
combined), the full Copilot runtime group (39 tests), the focused Bedrock test,
and both Antigravity persistence/request tests. Copilot declares its already
locked `serde` dependency directly for the new serialization view, adding no
new package version. The integrated locked CLI test-target check also passed.
These remove transient collection copies, not the useful catalogs themselves.
No provider calls or production RSS reduction were used as acceptance evidence.

The OpenRouter disk-cache writer's model copy is different: it publishes an
owned in-memory memo that survives the caller's borrowed slice. Removing it
requires an ownership/API redesign, not merely a borrowed write-only view.
Likewise, discovery returns that separately populate a retained cache are not
automatically redundant clones. Those paths were classified rather than
silently weakening caching or claiming the allocation was unnecessary.

### Scoped catalog and diagnostic ownership followup (2026-11-16)

Freshly parsed OpenAI/Anthropic catalog stores now transfer the selected scope
into hydration rather than cloning it. Context-limit population consumes its
owned map and moves new String keys into the cache. Account-scoped reasoning
and service-tier reads likewise move their owned maps out of the parsed store,
while preserving missing-versus-empty capability behavior and leaving disk
bytes unchanged. Background OpenAI refresh persists its borrowed snapshot first,
then transfers context limits and model IDs into the live caches. Hosted checks,
all 3 catalog ownership tests, and an offline refresh-completion test passed.
These are structural clone reductions, not measured CPU or RSS savings.

The shared scoped catalog writer was then investigated with a concrete borrowed
serialization overlay rather than dismissed for missing evidence. It retains
the original HashMap insertion/iteration layout, drops the replaced owned
scope, and serializes the replacement directly from borrowed collections. Exact
compact and pretty legacy bytes passed for existing/new scopes and 0/1/32/2048
models. Empty-input no-write behavior, timestamps, account isolation, and eight
concurrent scope updates also passed; the original write lock remains intact.

A hosted release experiment alternated old/new update-plus-serialization paths
for nine rounds using per-thread CPU time. Median old/new nanoseconds were:

| Fixture | Existing scope | New scope |
|---|---:|---:|
| Tiny, 1 model | 1,017 / 770 | 1,418 / 1,174 |
| Small, 32 models | 55,967 / 47,347 | 61,026 / 52,325 |
| Large, 2,048 models | 6,462,648 / 5,820,302 | 6,784,973 / 6,140,190 |

This supported accepting the overlay: the tested medians improved about 9–24%
while eliminating the temporary deep copy. This measures the in-memory operation,
not end-to-end disk latency, production CPU, peak memory, or RSS. Five hosted
catalog ownership/persistence tests passed with the optional CPU probe ignored;
the explicitly requested release probe passed separately. Reproduce with
`justrust test --release --locked -p jcode-base --lib scoped_writer_release_cpu_comparison -- --ignored --nocapture --test-threads=1`.

Debug job summaries/status no longer explicitly clone strings and optional
output/error fields before JSON serialization makes its required owned result.
Both hosted regression tests passed, covering all job states, null/empty values,
Unicode/escaping, large output, and source/result isolation. Provider doctor
reports similarly removed 28 explicit cloned JSON inputs. Its hosted check and
2 credential-free equality/source-ownership tests passed without running live
provider probes or changing their opt-in gating. These changes eliminate
transient intermediate copies, not the independent owned diagnostic response.
Adjacent swarm diagnostics also borrow the friendly name and git-common path
when building JSON, and transfer the consumed resolver path without cloning it.
The existing isolated swarm-write regression and hosted app-core test-target
check passed. A read-only `swarm:id` result was captured for comparison after
activation; no real notification was sent as a runtime test.

The bounded direct-JSON-input sweep also removed 13 explicit clones from
OpenAI/OpenRouter/Copilot missing-tool-output construction and TUI remote status
metadata. The resulting JSON still owns its values independently of the source.
Hosted checks and 17 focused tests passed across those four areas, including
multiple repaired tool outputs and null/empty/Unicode remote metadata. No live
provider request or real session mutation was used as a test. Legacy-oracle
clones in test code were deliberately retained.

The CLI test targets also passed the final hosted locked integrated check.

Model picker alias expansion additionally transfers each consumed original
String into its output rather than cloning it a second time. The separate
owned deduplication set remains unchanged. A pointer-identity regression checks
that the first unique strings retain their backing allocations and output
order while empty and duplicate entries are handled as before. The final
catalog ownership suite passed 6 tests with the optional CPU probe ignored.
This is an allocation-copy reduction, not a measured production RAM saving.

### Schema, reranking, and retry ownership acceptance (2026-11-16)

The MCP schema cache now shares immutable catalog backing between memoized
loads and snapshots. Updating or pruning one snapshot detaches its map while
retaining unchanged servers' schema backing. A synthetic catalog with more
than 1,000,000 serialized bytes verified shared backing, unchanged-entry sharing,
mutation isolation, byte-identical legacy JSON, and memo invalidation on path
and file changes. All 11 scoped MCP cache tests passed. The production cache
file observed during investigation was 892,678 bytes, which is serialized size,
not its heap usage or a production RAM saving.

Memory reranking formats candidate contents through borrowed iterators rather
than cloning full candidate IDs and contents before truncation. Concurrent
consensus judges borrow the same prompt and sidecar instead of retaining one
owned copy per vote. All 16 scoped reranking tests passed, including exact
legacy prompt equality for empty, Unicode, multiline, and truncation-boundary
inputs. Voting, parallelism, error handling, and selection policy are unchanged.

Jev retries and concurrent voice hedges share immutable serialized bytes instead
of copying the complete wire Vec per attempt. A backing-identity test verified
that 16 transport bodies all reference the original 64 KiB allocation, including
after the original Bytes handle is dropped. A separate loopback test verified
byte-identical requests larger than 64 KiB across 503, 529, and successful
responses. The existing 80 KiB context bound, retry budget, authentication,
entitlement, and hedge timing remain unchanged. The scoped Jev suite passed
51 tests with one ignored.

These are ownership and exact-behavior results, not production RSS reductions
or matched-workload latency measurements. Hosted admissions initially stalled
with an absent build machine, so these suites used an explicit, documented
justrust local fallback and a reused test harness. Supported hosted provisioning
subsequently recovered. No allocator policy, history cap, delivery delay, cache
TTL, or useful retained state was changed.

The sidecar's owned OpenAI JSON and both Claude typed request wrappers now
drop after eager serialization, before network/response waits. All 21 sidecar
tests passed on the recovered hosted service, including loopback API/SSE
success/error requests with exact JSON and authentication headers, and
credential-free Claude body serialization after request-wrapper release.

WebSocket prewarm jobs and their background tasks now share one immutable
settings tree. A large tools/instructions fixture verified common backing,
unchanged serialization, caller-mutation isolation, equivalent-request reuse,
and replacement cancellation without making a network connection. Unchanged
requests avoid even the new Arc allocation. The full hosted native runtime
suite passed 237 tests with three ignored. Speculation, cancellation, the
30-second TTL, and foreground behavior were not changed.

Gmail integration requests release owned JSON after eager serialization, and
successful Composio responses move their `data` out of the discarded envelope
instead of cloning its tree. Thirteen hosted Gmail tests passed, including
25 dummy-auth loopback exchanges for exact request JSON/headers, data shapes,
missing/null data, and error precedence. No real email or external API was
used. These lifecycle/move changes likewise have no measured production RSS
or end-to-end latency claim.

Mobile live-session heartbeats now serialize borrowed session views directly,
without an intermediate Vec of JSON maps, copied session identifiers, or a
second wrapper JSON tree. All ten hosted local-runner tests passed, including
exact legacy wire bytes for empty, Unicode, and 90-session loopback reports.
Polling intervals, report limits, and delivery behavior are unchanged.

Telegram notification and long-poll parameters similarly use borrowed/stack
serialization views instead of JSON maps. All four hosted tests passed.
Exact byte comparisons cover empty and large Unicode messages after their
source strings are dropped, omitted/present/negative offsets, and unchanged
long-poll timeouts. Tests build requests only and send no real messages.

The OpenAI-compatible embedding backend serializes its original input strings
through a borrowed view, then releases those owned inputs before its blocking
network wait. It no longer builds an additional owned JSON input tree or
copies the model/key for a scoped thread. All six hosted backend tests passed
with both default and no-default features. Loopback tests verify legacy bytes,
headers, vector-index ordering, errors, and the no-network empty-input path.
No model is downloaded and no external embedding request is made by tests.
These are structural allocation/lifetime gains, not measured production RSS
savings. Client/thread policy, timeout, output, and dimension checks are intact.

The follow-up inventory also distinguished non-candidates: the memory graph
cache helpers currently have no callers, so they do not establish live waste.
The config reload cache returns static references and intentionally preserves
old allocations; freeing them requires a sound ownership/API redesign, not an
unsafe early drop. Neither finding proves that wider optimization is impossible.

### Native response-wait ownership acceptance (2026-11-15)

The native OpenAI runtime previously held its separately built input array after
the persistent-continuation attempt, even though the complete retry request
already owned the input. It now releases that redundant array before fresh
HTTPS/WebSocket attempts. Continuation-only diagnostic payload and incremental
input copies are also released after fingerprinting, before response waits.

Two matched, pinned, local-only native HTTPS workloads resumed sixteen sessions
with 1 MiB of tool output each. Each receiver asserted the `/v1/responses` path,
native input shape, and fixture model. Live allocator growth was
**73.3776/73.3663 MiB before versus 57.2281/57.1410 MiB after**, a paired reduction
of **16.1495/16.2254 MiB**. This is not a fixed process-RSS or production-workload
savings claim. Earlier attempts accidentally restored an OpenAI-compatible
route, and a subsequent attempt exposed a fixture catalog mismatch. Those runs
were discarded rather than used as native evidence.

The full native runtime suite passed **232 tests, 3 ignored**, serially. Parallel
execution exposed an environment-sensitive catalog endpoint test failure, so
that run is not reported as passing. Four retry regressions then passed,
including a new real HTTP test asserting the complete 1 MiB prompt on every
attempt after a provider overload. Retry, cancellation, fallback, and delivery
policies remain unchanged. Anthropic/OpenRouter diagnostic borrowing separately
passed 317 tests, 2 ignored, with exact schema/hash/null/count checks.

Gemini and Antigravity diagnostic JSON now lives entirely in a synchronous
logging helper, with serialized fields moved into one payload and borrowed for
fingerprinting. The helper returns before HTTP response waits. All 74 runtime
tests passed, including exact legacy JSON/hash/optional-field equivalence.
This establishes shorter ownership lifetimes, not a measured RSS reduction.

A 4 KiB bounded hashing writer was rejected: optimized paired measurements
increased CPU by 26% for small requests, 31% for many tiny fields, and 16% for
large escaped Unicode. Production hashing remains unchanged. Removing a
serialization allocation does not qualify as a no-tradeoff gain when it adds
that CPU cost. Reproduction artifacts are retained separately from ordinary
tests, without installing a test-wide allocation interceptor.

The next ownership pass removed duplicate Cursor prompt/system diagnostic
values (54 runtime tests passed) and moved Copilot input/messages/tools into
the actual request tree, borrowing diagnostics from it (38 tests passed).
Four Responses stream dequeue paths now move queued events instead of cloning
and discarding the original. Nine parser tests passed, including a large
payload allocation-identity and ordering check. Browser result and parsed tool
input extraction similarly move JSON out of discarded envelopes, with fourteen
web tests passing, including large nested payloads and missing-versus-null
behavior. These are copy-elimination guarantees, not measured production RSS
savings or changes to history, retry, cache, or delivery policies.

### Isolated allocation-stack follow-up (2026-11-15)

Two private jemalloc-prof workloads were run after activating the ownership
fixes. This instrumented allocator was never installed in production. Each
workload used a private home/socket, `JCODE_TEST_SESSION=1`, fake credentials,
and a route-asserted localhost Responses server with sixteen 1 MiB transcripts.
The first zero-session heap-dump attempt failed because allocator commands
require a session. It was discarded and both corrected runs completed.

Sampled live-heap reports were **62.6/62.7 MiB active** and **24.9/24.7 MiB**
five seconds after local responses completed. Allocation-origin stacks showed
about **20.0 MiB** in wire JSON serialization, **16.2/16.6 MiB** in persisted
message parsing, and roughly **16 MiB** in request JSON construction. Cumulative
frames overlap and must not be summed indiscriminately. These figures are
sampled heap estimates, not exact allocator counters, production RSS, or a
comparison between allocators. History and retry payloads remain necessary;
wire shrinking and bounded hashing have already failed no-tradeoff acceptance.

A smaller **1.2–1.6 MiB** sampled pricing-cache publication origin exposed
another avoidable transient deep clone: the fresh cache was cloned into an Arc
before the original was dropped. The cache is now consumed and published in an
Arc without cloning its backing storage. Ten serial pricing tests passed,
including allocation identity, byte-identical disk JSON, lookup/reload, and
failed-write publication. Retention, refresh frequency, and pricing are
unchanged. This avoids a transient copy, not two permanently retained caches.

Artifacts and reproduction notes are in `~/.jcode/scratch/ram-fork/HEAP_STACKS.md`,
`heap-stack-probe.py`, and `heap-stacks-2` / `heap-stacks-3`. Both private daemons
and fixture sockets were cleaned up. The production allocator, history, and
API bridge were unchanged. This synthetic workload does not explain the entire
production heap or prove that all theoretical optimization is exhausted.

### Export and redaction ownership acceptance (2026-11-15)

Unchanged export redaction now retains shared message payloads. Changed fields
are computed once before a single copy-on-write detach, preserving the previous
redaction output, opaque fields, and original-session isolation. An independent
eager-redaction oracle covers every content variant and nested JSON values.

An isolated allocator probe retained eight export snapshots of sixteen
106,496-byte text messages and one 2 MiB opaque image. Live allocated-byte growth
was **30,505,008 before versus 39,104 after** for safe content, and
**30,480,416 versus 13,701,392** when every text message needed redaction.
These are retained allocator measurements for this fixture, not production RSS.
Single-run elapsed times were 176,158/166,226 microseconds for safe exports and
693,024/687,604 for changed exports. They do not establish a general speedup.
Reproduce serially with
`justrust test -p jcode-base --lib export_sharing_cpu_allocator_probe -- --ignored --nocapture --test-threads=1`.

Separately, regex no-match replacements now reuse the existing string instead
of converting a borrowed `Cow` into another full copy. A 1,107,000-byte helper
probe performed fifty no-match replacements with **50 allocation replacements
before and zero after**, preserving exact output. Existing cache breakpoints
also no longer detach shared message payloads just to retain the same marker.
These are allocation/copy reductions, not measured process-RSS reductions.

### Rejected wire shrinking candidate (2026-11-15)

The tested candidate shrank wire buffers only when spare capacity was at least
1 MiB and at least half their length. Real localhost uploads covered off/gzip/
zstd, repeated and varied input, 1/8/16 MiB fixtures, slow readers, cancellation,
and response waits. All 216 transport runs and 36 exact receiver-byte checks
passed. Fifteen of eighteen fixtures saved 1–16 MiB of allocation capacity, but
there was **no meaningful peak-memory or consistent RSS benefit demonstrated**.
Timing noise also left latency neutrality unproven. The shrinking policy was
therefore rejected rather than deployed to improve capacity counters alone.

Only the simpler early release of the no-longer-needed raw serialization buffer
after compression was retained, with off/gzip/zstd HTTP byte/header equivalence
coverage. This shortens a local allocation lifetime, not a demonstrated
steady-state or peak-RSS gain. No compression, retry, cache, or history policy
was changed. The remaining live-heap attribution gap still requires allocation
stack evidence before it can be classified as redundant ownership.

### Shared compaction acceptance example (2026-11-15)

Sixteen resumed sessions, each with a 1 MiB encrypted compaction payload, grew
live allocator usage by **52.358–52.729 MiB before** and **36.578–36.702 MiB after**
sharing immutable session/persistence snapshots, over two pinned runs per binary.
The pinned revisions also include concurrent changes, so this is a workload
comparison rather than a one-patch benchmark. Ownership tests and payload
counters independently verify the eliminated metadata copy.
Process-private growth varied more, so this is not a fixed RSS savings claim.
Both candidate runs retained 16,777,744 bytes of canonical payload capacity and
zero additional persistence-metadata payload capacity. The independent compaction manager summary was initially a separate owner.
A subsequent narrower change shares only the immutable encrypted body between
manager summaries and stored states, retaining independently mutable summary text.

Borrowed exact comparison also avoids constructing an owned exported payload
solely to detect unchanged state. An optional probe of 128 comparisons with a
9 MiB payload took 67,228 microseconds for export-and-compare and 21,172 for the
borrowed comparison in one hosted test run. This measures that operation, not
application-wide latency or retained RSS. Run the focused probe with
`justrust test -p jcode-base --lib persisted_state_comparison_probe -- --ignored --nocapture`.

The follow-up owner pass found about 0.57 MiB combined AGENTS/skill snapshot
string capacity across 41 ready live sessions. A local completed-turn fixture
retained 26 default tools with 32,697 serialized JSON bytes, not a heap bound or
a measurement of custom MCP catalogs. Immediately post-reload, locked tool
snapshots are cold, so their zero count is not representative of warmed turns.
The approximately 10.3 MiB search index already shares incremental Bloom-filter
backing storage. No cache reduction was made to manufacture further savings.
The encrypted-body follow-up (`fe1773bb6`, sharing regressions in `63bf3a240`)
repeated the isolated sixteen-session workload with two runs per pinned binary.
Live-allocator growth fell from **36.261–36.400 MiB to 20.163–20.309 MiB**.
Private-page growth was also lower but noisier, so this is not a guaranteed
production PSS reduction. The pinned builds contain concurrent changes, while
pointer-sharing, copy-on-write and byte-identical JSON tests independently
verify the removed encrypted-body allocation. No history or cache was discarded.
At that stage, custom tool catalogs and transcript/provider-cache payload
copies remained separate candidates. Later rounds added MCP schema snapshot
sharing and shared transcript payloads with copy-on-write isolation, documented
above. Those fixes do not prove that every custom tool or request owner is
redundant, or that all remaining production heap is fully attributed.

## Hard caps

### Markdown cache budget

Source: `src/tui/markdown.rs`

| Metric | Budget | Why |
|---|---:|---|
| `highlight_cache_entries` | `<= 256` | Explicit cache cap (`HIGHLIGHT_CACHE_LIMIT`) |

Required review action if violated:
- explain why the cache limit changed
- update this doc
- update any affected tests or benchmarks

### Mermaid cache budget

Sources:
- `src/tui/mermaid.rs`
- `src/tui/mermaid_cache_render.rs`

| Metric | Budget | Why |
|---|---:|---|
| `render_cache_entries` | `<= 64` | Explicit render-cache cap (`RENDER_CACHE_MAX`) |
| `image_state_entries` | `<= 12` | Explicit protocol-state cap (`IMAGE_STATE_MAX`) |
| `source_cache_entries` | `<= 8` | Explicit decoded-source cap (`SOURCE_CACHE_MAX`) |
| `active_diagrams` | `<= 128` | Explicit active-diagram cap (`ACTIVE_DIAGRAMS_MAX`) |
| `cache_disk_png_bytes` | `<= 50 MiB` | Explicit on-disk cache cap (`CACHE_MAX_SIZE_BYTES`) |
| `cache_disk_max_age_secs` | `<= 259200` | 3-day expiry (`CACHE_MAX_AGE_SECS`) |

Required review action if violated:
- document the new limit and reason
- verify eviction still works
- verify no unbounded growth path was introduced

## Ratchet expectations

### Session and transcript memory

Source: `src/session.rs`, `src/tui/memory_profile.rs`

These are not strict caps yet, but they are expected relationships.

| Metric relationship | Expectation |
|---|---|
| `provider_messages_cache.count` vs `messages.count` | Should remain in the same order of magnitude for a single session, and normally track the transcript closely |
| `session_provider_cache_json_bytes` vs `canonical_transcript_json_bytes` | Should remain comparable for normal chat flows, not explode independently |
| `transient_provider_materialization_json_bytes` | Should return to zero or near-zero outside active materialization-heavy paths |
| `display_large_tool_output_bytes` | Large values require explanation because they usually mean raw tool output is being retained too aggressively in the UI |

Required review action if violated:
- show before/after memory profiles
- explain which retention path grew
- prefer fixing duplication before raising any budget

### Runtime memory log expectations

Source: `src/runtime_memory_log.rs`

Runtime memory logs are the regression detection mechanism, not just a debug feature.

Expected behavior:
- server/client logs should be sufficient to explain large changes in:
  - session/transcript totals
  - provider cache totals
  - TUI display totals
  - side panel totals
- new large memory owners should emit attributable signals instead of appearing only as unexplained RSS growth

Required review action if violated:
- add or improve attribution before accepting the memory increase

## Review checklist for memory-affecting changes

When changing memory-heavy code, capture and include:

1. Which counters changed?
- aggregate `:debug memory`
- targeted `:debug markdown:memory` / `:debug mermaid:memory`
- `agent:memory` when session/provider cache behavior changes

2. Was a hard cap changed?
- if yes, explain why the old cap was insufficient

3. Did duplication increase?
- canonical transcript
- provider cache
- materialized provider view
- display copy
- side-panel copy

4. Did observability remain adequate?
- if memory grew, can logs/profiles explain where?

## Current initial budget summary

These are the concrete enforced limits today:

- Markdown highlight cache entries: 256
- Mermaid render cache entries: 64
- Mermaid protocol image-state entries: 12
- Mermaid decoded source-cache entries: 8
- Mermaid active diagrams: 128
- Mermaid on-disk PNG cache: 50 MiB, max age 3 days

Any intentional change to those limits must update this document in the same PR.

## November 15, 2026 ownership follow-up

- Message and stored-message payloads now share immutable backing storage and
  detach on mutation. Legacy JSON arrays and independent snapshot mutation are
  preserved. An isolated 32-snapshot allocator probe over a 720,896-byte body
  measured 23,080,144 bytes of growth for owned-vector copies versus 3,440 bytes
  for shared snapshots. This is an ownership probe, not production RSS savings.
- Pinned 16-session local-provider workloads did not establish a steady-state
  streaming RAM reduction from transcript sharing alone. Provider transports
  already release request-preparation transcript snapshots before network waits.
- The OpenAI-compatible/OpenRouter retry transport separately retained a full
  JSON request and a deep per-attempt clone. The attempt now borrows the retry
  owner's immutable request, preserving serialization and retry behavior.
  Two private 16-session runs with 1 MiB of tool results per session measured
  allocator-live growth of 74,415,664 and 74,265,760 bytes before, versus
  57,186,656 and 57,134,672 bytes after. The second pair reversed run order.
  These are isolated real transport measurements, not a claim about current
  production totals or a fixed RSS reduction.
- The retry-body change passed 12 wire tests, 11 retry-filter tests and four
  Azure retry tests on hosted builds. No cache limits, history retention,
  batching, timers or output quality were reduced.
- The same borrowing rule now applies to Anthropic attempts and native OpenAI
  HTTPS/opaque-recovery attempts. Hosted validation passed 118 Anthropic runtime
  tests (one ignored), four native HTTP idle tests and ten opaque recovery tests.
  These provider equivalents have structural copy-removal evidence, not their
  own measured production RAM delta. The integrated session suite additionally
  passed 143 tests (six ignored).
- Follow-up ownership review found that transformed websocket wire trees have
  no uses after send and timeout-policy derivation. Releasing these temporary
  values before response reception preserves the caller's retry request and
  persistent response state. This is a lifetime improvement, not a measured RSS
  claim. The original retry request, canonical transcript and useful caches
  remain deliberately retained. Eliminating those owners requires additional
  attribution or a wider serialization/ownership redesign, not arbitrary caps.
  The early-release change passed the full native OpenAI runtime suite:
  232 tests passed, three ignored, including wire, cancellation and recovery.

## November 15, 2026 deeper attribution

A read-only production pass at 13:03 UTC profiled 52 of 53 sessions without
forcing busy-agent locks. Three subsequent samples reported allocator-live
713,265,168–715,795,920 bytes and anonymous PSS 946,536,448–949,792,768 bytes.
The existing resident-retention estimate was 230,740,528–236,527,600 bytes.
This is not proof that those bytes are immediately reclaimable. GNU malloc's
free-chunk counter includes address space whose physical pages may already have
been returned. Concurrent activity and diagnostic allocations also prevent a
controlled production savings claim. No production purge was performed.

Two isolated, no-inference 16-session lifecycle runs used the installed binary,
dedicated session storage and private sockets. After request completion, an
explicit private-only allocator purge reduced private resident memory by
18,169,856 and 18,489,344 bytes, with nearly unchanged live allocation counts.
Active-request reductions were only 3,194,880 and 3,350,528 bytes. Existing
completion trimming may already have run before the manual purge. These results
do not justify increasing production trimming frequency without latency and
power measurements.

A separate copied-source request-compression probe ran 36 isolated processes:
1/8/16 MiB payloads, repeated and deterministic-hex content, raw/gzip/zstd,
twice each. Paired requested-allocation and capacity counters matched exactly.
For a 16 MiB payload, raw JSON length 16,777,259 bytes occupied a Vec capacity
of 33,554,482 bytes. Repeated-content gzip produced 76,586 wire bytes while
retaining capacity 8,388,629 bytes. Compressed paths released the raw allocation
before `json_body` returned, but the wire allocation persisted until request
drop. Thus wire length substantially understates retained buffer capacity.
These are Rust allocation counts, not RSS or real upload-completion measurements.
Native codec allocations, malloc overhead and realloc transient overlap are
excluded. Buffer compaction is a candidate, not an accepted latency-free gain.

Reproduction artifacts are under `~/.jcode/scratch/ram-fork/` (production
`deep-measure`, isolated `allocator-lifecycle-3` and `allocator-lifecycle-4`)
and `~/.jcode/scratch/request-buffer-lifetimes/` (README, full measurements,
validation and locked scratch build). Serialized content lengths still do not
explain all production live heap. Capacity accounting must deduplicate shared
payload backing and explicitly exclude unmeasured map-node/allocator overhead.

Additive retained-capacity diagnostics passed 31 message-type tests and three
session profile tests, then were activated as `7d05ba673`. A subsequent read-only
pass profiled 46 of 50 sessions. The sum of known transcript backing capacity
was 82,889,738 bytes: 70,010,226 string bytes plus 12,879,512 Vec bytes. This
sum deduplicates within each session, not across sessions, and excludes four
busy/unavailable sessions, JSON map nodes, Arc/allocator overhead and other
owners. The sampled strings had no spare capacity after reload. Production
allocator-live remained 553,280,576–567,569,856 bytes. These unmatched scopes
cannot explain the entire heap or establish that the difference is waste.
Do not attribute the lower post-reload process totals to the diagnostics.
The API bridge retained PID 310062 throughout activation.

A scratch-only wire-buffer `shrink_to_fit` comparison checked 18 case pairs and
matched wire checksums, lengths and requested-allocation reductions. For a
16 MiB payload it removed 16,777,223 spare raw-buffer bytes, or 8,312,043 bytes
for repeated-content gzip. Ten alternating timing pairs per case measured
shrink-call medians ranging from 105 ns to 32,058 ns, with noisy overall timing
deltas, including positive paired deltas. This demonstrates allocation savings,
not a universally free optimization, physical-page savings, or unchanged real
upload latency. No wire-buffer compaction or allocator policy change was
installed in this measurement round. Raw result matrices are retained in the
scratch `compaction-results.json` and `compaction-summary.txt`.
