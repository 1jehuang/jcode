# CVP Dossier Sections 4-6: Comparator, Pilot, Measurements

Applies to all three primary repos per docs-3 dossiers.

---

## Section 4: Comparator and Reuse Hypotheses

### Pinned Baselines (to verify and version-pin)

| Baseline | Repo | Pin | Status |
|----------|------|-----|--------|
| KCode owned build | KooshaPari/jcode | v0.85.1-k1.1.0 | Built, tested 16/16 |
| HeliosLite owned build | KooshaPari/HeliosLite | v2.13.21-h.0.2.1 | Built, tested 429/430 |
| HeliosCLI owned build | KooshaPari/HeliosCLI | v0.2.1 | Built, tested 28/28 |
| Upstream ForgeCode | tailcallhq/forgecode | NEEDS PIN | Not cloned |
| Upstream Codex CLI | openai/codex | NEEDS PIN | Not cloned |
| Upstream jcode | 1jehuang/jcode | NEEDS PIN | Not cloned |
| Direct model/tool runner | N/A (design stub) | N/A | Not implemented |

### Fork Deltas (what each repo adds over upstream)

**HeliosLite over ForgeCode:**
- SQLite session store with FTS5 search + zstd compression
- Subagent lifecycle management (find/rebind/detach)
- ShareCLI realtime relay
- backoff + circuit breaker + bulkhead resilience
- HERDR terminal runtime integration
- Brand/identity: 5 binaries (forge, forge-dev, helioslite, helioslite_helper, forgecode)

**HeliosCLI over Codex:**
- Reimplemented harness workspace (28 crates, NOT using vendored codex)
- helios-ai: OpenAI/Ollama/LMStudio provider adapter
- helios-sandbox: Landlock (Linux) / Seatbelt (macOS)
- harness_checkpoint: git-based checkpoint/rollback
- kla: CLI recording/screenshot tool

**KCode over jcode:**
- HERDR terminal detection + reporter (socket-based state machine)
- Fork identity: v0.85.1-k1.1.0, herdr-kind CLI arg

### Reuse Decisions (per dossier requirement)

| Subsystem | Decision | Rationale |
|-----------|----------|-----------|
| Upstream provider clients | RETAIN | Protocol-compatible, shared API surface |
| Session persistence | RETAIN (HeliosLite) | SQLite+FTS5 is proven, well-tested |
| Checkpoint mechanism | RETAIN (HeliosCLI) | Git-native is simple and auditable |
| Resilience layer | RETAIN (HeliosLite) | backoff+circuit breaker prevents cascade |
| Sandbox | RETAIN (HeliosCLI) | OS-level enforcement, not app-level |
| ShareCLI | RETAIN (HeliosLite) | Owned subsystem, no upstream equivalent |
| HERDR | RETAIN (KCode) | Owned subsystem, no upstream equivalent |
| Vendored codex-rs | KEEP AS REFERENCE | Not built, excluded from workspace |

---

## Section 5: Controlled Pilot Design

### Per-Repo Pilot Recipe

All pilots use the same long-running coding + transport-fault corpus as defined in the KCode dossier.

**Common corpus:**
- Multi-step repository task (create file, modify, test, commit)
- Parent agent spawns child agent for subtask
- Inject: stream disconnect, rate limiting, delayed tool response
- Inject: failed child task, parent recovery
- Inject: process restart mid-task
- Measure: state recovery, no duplicate mutations, no unauthorized writes

**HeliosLite-specific additions:**
- Legacy/foreign session import and search
- Switch project roots mid-session
- ShareCLI relay under load
- Subagent detach/promote lifecycle

**HeliosCLI-specific additions:**
- Checkpoint at task midpoint, rollback to checkpoint
- Permission-denied filesystem action (sandbox enforcement)
- Model swap (OpenAI -> Ollama) mid-conversation
- Upgrade fork, confirm identity and session compatibility

**KCode-specific additions:**
- HERDR reporter state transitions under fault
- Protected file + forbidden repository remain present after resume
- Actual installed build + updater path

### Pilot Execution Protocol

1. Pin exact tool versions (jcode, helioslite, helios CLI)
2. Clone fixture repository at fixed commit
3. Run each repo's binary against identical task script
4. Record: time, memory, CPU, mutations, errors
5. Run negative-control cases (forbidden writes, boundary crossings)
6. Retain all logs, screenshots, intervention records

---

## Section 6: Measurements and Claims

### Per-Repo Advantage Hypothesis + Metrics

**KCode:**
- Claim: "Sustained useful operation at lower memory and recovery burden, with stronger mutation safety"
- Metrics:
  - Task completion rate (correct output / total attempts)
  - Forbidden-action rate (writes outside allowed scope)
  - Peak memory at fixed workload
  - Manual recovery count after injected faults
  - State loss / duplicate side effects
  - Patch carry cost per upstream update

**HeliosLite:**
- Claim: "Connection continuity and usable session recovery on actual endpoints, with scope and provenance preserved"
- Metrics:
  - Completion rate after stream disconnect/reconnect
  - Session search correctness (FTS5 results match expected)
  - Latency tails under disconnect/reconnect
  - Operator intervention count
  - Clean installation/update fidelity
  - Forbidden writes across workspace boundaries

**HeliosCLI:**
- Claim: "A small useful compatibility/workflow delta that justifies maintaining the fork"
- Metrics:
  - Task success rate with restriction compliance
  - Setup and maintenance effort (time to first successful run)
  - Provider portability (OpenAI -> Ollama swap)
  - Resumption correctness after checkpoint/rollback
  - Artifact size and startup time
  - Owned patch divergence from upstream

### Measurement Instruments

| Instrument | Purpose | Status |
|------------|---------|--------|
| `time` / `/usr/bin/time` | Wall + CPU time | Available |
| `valgrind --tool=massif` or `heaptrack` | Memory profiling | NEEDS INSTALL |
| Custom mutation tracker | Log all file/system mutations | NOT BUILT |
| HERDR reporter | State transitions under fault | Built (KCode) |
| Test fixtures | Fixed task scripts | NOT BUILT |

### Blocking Items for Pilot Execution

| Blocker | Impact | Resolution |
|---------|--------|------------|
| Upstream repos not cloned | Cannot build comparator baselines | Clone tailcallhq/forgecode, openai/codex, 1jehuang/jcode |
| Fixture repo not created | Cannot run controlled tasks | Create minimal coding task fixture |
| Mutation tracker not built | Cannot measure forbidden writes | Build or reuse existing audit tool |
| Memory profiler not installed | Cannot measure memory | Install heaptrack or use jemalloc profiling |
| No VHS/screenshot tool | Cannot qualify authentic UI | Install vhs for terminal recordings |

---

## Status

| Section | Status | Blocking |
|---------|--------|----------|
| 4. Comparator baselines | PARTIAL — owned builds pinned, upstreams not cloned | Clone upstream repos |
| 5. Controlled pilot design | DONE — recipes defined per repo | Fixture repo + mutation tracker |
| 6. Measurements spec | DONE — metrics and instruments defined | Upstream clones + profiler install |
| **Overall 4-6** | **DESIGN COMPLETE, EXECUTION BLOCKED** | Upstream repos + fixtures |
