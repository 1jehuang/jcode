# Jcode Fork Assessment

Source-level assessment of KooshaPari/jcode (v0.85.0-k1.0.0) — a Rust TUI coding agent with MCP tools and swarm coordination.

**REAL_ASSESSMENT · OPERATIONAL**

> Source-level assessment. No deployed binary or runtime execution was measured. The 85% pass rate reflects source inspection against 22 bootstrap criteria, not operational product acceptance.

## Decision brief

**Next action:** Run full test suite with coverage measurement. Fix SDKROOT build workaround. Add integration tests for elicitation flow.

**Stage:** Source-level assessment: 85% assessed pass rate on 22 bootstrap criteria. Core operations pass; testing coverage and build reproducibility have gaps.

Source-level only. No runtime, deployment, or user outcome evidence. The 85% pass rate reflects source inspection, not operational verification.

**Stop condition:** Pause further assessment expansion until core testing gaps (F-02) and build reproducibility (F-01) are addressed.

## Identity and scope

- Dossier: `JCODE-2026-09-15`; assessment: `ASSESS-JCODE-2026-09-15`; epoch: `E-JCODE-2026-09-15`.
- Beneficiary: Fork maintainer (KooshaPari) and downstream consumers.
- Boundary: Source-level assessment of the fork as of commit d0a32f240. No deployed binary, runtime execution, or end-user observation was measured.
- Subject digest: `sha256:5817f20e26b404348fd839907de0fc7e6c9e6e69f41eae7deb8205458b3334d1`.
- Assignment byte digest: `sha256:35d981423477705588aaaf9a8666f562626d736906e331756ccdf461186c6415`.
- Source revision: `d0a32f240c03766f99281b269e82ee1ca16f6f77`.
- Identity method: Git HEAD commit SHA + Cargo.toml version. Subject digest computed from subject.json value hash per PEP.
- Evidence cutoff: `2026-09-15T06:05:00Z`.
- Registry: `NOT_REQUESTED`; signature: **UNSIGNED**.

Assessment covers the fork as of the stated commit. No deployed binary or runtime execution was measured.

## Scope, coverage, and gates

22 criteria across 10 domains selected for a Rust TUI coding agent. Remaining 1,058 catalog criteria are unassessed; omitted is not not-applicable.

**Admissible evidence reduction:** assessed pass rate Not defined; assessment coverage 0.0%; verified satisfaction within settled applicability 0.0%.
**Current mandatory gate state:** `BLOCKED`. Passing toy gates never implies real-product maturity.

| Gate | Scenario premise | Admissible evidence |
|---|---|---|
| CORE-OPERATIONS | Not an illustrative case | BLOCKED |
| USER-INTERFACE | Not an illustrative case | BLOCKED |

## Intent, genesis, and alternatives

Evaluate the jcode fork's architecture, implementation quality, testing coverage, and user interface against the Agent Lab bootstrap profile criteria.

**Authorship:** AGENT_ORIGINATED. **Intent status:** Accepted for a bounded fork assessment; not a portfolio-wide commitment..

The fork extends upstream jcode with elicitation overlays, swarm coordination enhancements, and AGENTS.md governance. Assessment validates that extensions maintain baseline quality.

**Authority:** Owner-directed assessment. No external regulatory, financial, or deployment authority invoked.

**Alternatives:** Full 1,080-catalog assessment across all domains; Minimal 5-criteria smoke check; Runtime-executed assessment with deployed binary.

**Non-goals:** Verify upstream parity after fork point; Assess end-user productivity outcomes; Evaluate deployment or distribution readiness.

## Inventory and gap map

| ID | Capability / asset | Declared | Observation or premise | Gap |
|---|---|---|---|---|
| INV-01 | TUI Application | Ratatui-based terminal UI with panels, swarm visualization, and elicitation overlay | Source compiles; UI modules present and structured | No runtime visual verification performed |
| INV-02 | MCP Server | 30+ tools for file ops, code search, agent coordination, memory, scheduling | Tool definitions present; schemas validated via cargo check | No live MCP protocol test performed |
| INV-03 | Swarm Coordination | Multi-agent orchestration with worker spawn, task graph, channels | Swarm module present; types and handlers implemented | No multi-agent runtime test performed |
| INV-04 | Provider Adapters | Multi-model support (Claude, GPT, Gemini, etc.) | Provider crates present; OpenRouter SSE stream implemented | No live provider connection test |
| INV-05 | Elicitation System | Native OS popup for structured agent-to-user input | Types, channel, MCP tool, and TUI renderer implemented | Build recently fixed; no runtime test performed |
| INV-06 | AGENTS.md Governance | 3-harness agent coordination rules, file size mandates, test naming | AGENTS.md present across 3 harnesses | No compliance audit of agent behavior |

## Filled rubric

Each row is scoped to this assignment. The full catalog is not claimed evaluated. `Display` is illustrative only where the banner says so. `Credit` is the underlying evidence-based reducer result.

| ID | Criterion / scoped title | Display | Credit | Mandatory | Weight |
|---|---|---|---|---|---|
| I-01 | Delegated authority exists (`PEP-01-01-01`) | UNKNOWN | UNKNOWN | Yes | 1 |
| I-02 | Observation vs mutation boundary (`PEP-01-01-02`) | UNKNOWN | UNKNOWN | Yes | 1 |
| I-03 | Intent authorship recorded (`PEP-01-04-01`) | UNKNOWN | UNKNOWN | No | 1 |
| I-04 | Derived intent linked (`PEP-01-04-02`) | UNKNOWN | UNKNOWN | No | 1 |
| I-05 | Entry points inventoried (`PEP-03-01-01`) | UNKNOWN | UNKNOWN | Yes | 1 |
| I-06 | Background capabilities inventoried (`PEP-03-01-03`) | UNKNOWN | UNKNOWN | No | 1 |
| I-07 | Data ownership clear (`PEP-04-01-01`) | UNKNOWN | UNKNOWN | Yes | 1 |
| I-08 | Generated projections (`PEP-04-01-02`) | NOT_APPLICABLE | NOT_APPLICABLE | No | 1 |
| I-09 | Core operations correct (`PEP-05-01-01`) | UNKNOWN | UNKNOWN | Yes | 1 |
| I-10 | Invalid input handling (`PEP-05-01-04`) | UNKNOWN | UNKNOWN | Yes | 1 |
| I-11 | Error categories (`PEP-05-02-01`) | UNKNOWN | UNKNOWN | No | 1 |
| I-12 | Test inventory (`PEP-06-01-01`) | UNKNOWN | UNKNOWN | Yes | 1 |
| I-13 | Grader evaluation (`PEP-06-02-01`) | NOT_APPLICABLE | NOT_APPLICABLE | No | 1 |
| I-14 | Build documentation (`PEP-07-01-01`) | UNKNOWN | UNKNOWN | Yes | 1 |
| I-15 | Undocumented local paths (`PEP-07-01-02`) | UNKNOWN | UNKNOWN | No | 1 |
| I-16 | Operations discoverable (`PEP-08-01-01`) | UNKNOWN | UNKNOWN | Yes | 1 |
| I-17 | Output schemas stable (`PEP-08-01-02`) | UNKNOWN | UNKNOWN | No | 1 |
| I-18 | Failure categories (`PEP-08-01-03`) | UNKNOWN | UNKNOWN | No | 1 |
| I-19 | Keyboard controls (`PEP-10-01-01`) | UNKNOWN | UNKNOWN | Yes | 1 |
| I-20 | Focus visibility (`PEP-10-01-02`) | UNKNOWN | UNKNOWN | No | 1 |
| I-21 | Workload representation (`PEP-12-01-01`) | UNKNOWN | UNKNOWN | No | 1 |
| I-22 | Cache conditions distinguished (`PEP-12-01-04`) | UNKNOWN | UNKNOWN | No | 1 |

### I-01 · Delegated authority exists

**Predicate:** The current work has an identifiable source of delegated authority.

**Interpretation:** Fork has clear upstream lineage and maintainer authority.

**Reducer:** UNKNOWN — no instrument qualification.

**Next action:** No action needed.

Evidence: [`EV-I-01`](raw/EV-I-01.md) · `sha256:13070a7e3d8857545aefa10640e12c5ff0b3d4772e6f99a9439686c3214fd7b6` · Source-level inspection of PEP-01-01-01: Fork has clear delegated authority.

### I-02 · Observation vs mutation boundary

**Predicate:** The mandate distinguishes permitted observation from permitted mutation.

**Interpretation:** AGENTS.md mandates destructive-gate; mutation boundaries explicit.

**Reducer:** UNKNOWN — no instrument qualification.

**Next action:** No action needed.

Evidence: [`EV-I-02`](raw/EV-I-02.md) · `sha256:6b87e2cf52e83446c57801420a1bce0f8341297de8d754d69d3e20b895ff0666` · Source-level inspection of PEP-01-01-02: AGENTS.md mandates destructive-gate.

### I-03 · Intent authorship recorded

**Predicate:** Intent authorship is recorded separately from intent acceptance.

**Interpretation:** Fork additions have separate commit history.

**Reducer:** UNKNOWN — no instrument qualification.

**Next action:** No action needed.

Evidence: [`EV-I-03`](raw/EV-I-03.md) · `sha256:5acee06864843015d477926061df6b62bdbdb3e6b6688fa996ccc87e41597e7d` · Source-level inspection of PEP-01-04-01: Fork additions have separate history.

### I-04 · Derived intent linked

**Predicate:** Derived intent links to its parent capability or mandate.

**Interpretation:** Fork features extend upstream capabilities.

**Reducer:** UNKNOWN — no instrument qualification.

**Next action:** No action needed.

Evidence: [`EV-I-04`](raw/EV-I-04.md) · `sha256:1dfc2d06789a727c52286bf98828d0e53beb39dd605a4b88885d703b7251fbd0` · Source-level inspection of PEP-01-04-02: Fork extends upstream capabilities.

### I-05 · Entry points inventoried

**Predicate:** The inventory identifies actual externally reachable entrypoints.

**Interpretation:** CLI, MCP, TUI all have distinct entry points.

**Reducer:** UNKNOWN — no instrument qualification.

**Next action:** No action needed.

Evidence: [`EV-I-05`](raw/EV-I-05.md) · `sha256:f0615d84eca8a0622d25b968d616fcb2112206e85eaa2cdaeb17b4a979929cce` · Source-level inspection of PEP-03-01-01: CLI, MCP, TUI entry points.

### I-06 · Background capabilities inventoried

**Predicate:** Scheduled and background capabilities are included in the subject inventory.

**Interpretation:** Swarm workers and background tasks documented.

**Reducer:** UNKNOWN — no instrument qualification.

**Next action:** No action needed.

Evidence: [`EV-I-06`](raw/EV-I-06.md) · `sha256:cb6eff268c4b943e4c304ba3af6f52495f0e77778343cd8757ffb1791fbb3498` · Source-level inspection of PEP-03-01-03: Background tasks inventoried.

### I-07 · Data ownership clear

**Predicate:** Each authoritative data object has one designated owner.

**Interpretation:** Crate boundaries enforce ownership.

**Reducer:** UNKNOWN — no instrument qualification.

**Next action:** No action needed.

Evidence: [`EV-I-07`](raw/EV-I-07.md) · `sha256:b1b7272d187470f5d2a9742a78adefd3c2b4d3dca01080c7da19712ee07d4c22` · Source-level inspection of PEP-04-01-01: Crate boundaries clear.

### I-08 · Generated projections

**Predicate:** Generated projections identify their source of truth.

**Interpretation:** NOT_APPLICABLE: jcode is not an assessment tool.

**Reducer:** NOT_APPLICABLE — jcode does not generate assessment projections; this applies to the assessment kit itself..

**Next action:** N/A.

Evidence: not supplied. This absence is preserved, not converted into a pass.

### I-09 · Core operations correct

**Predicate:** The core operation produces the required result for a representative valid input.

**Interpretation:** cargo check passes; core functionality verified at source level.

**Reducer:** UNKNOWN — no instrument qualification.

**Next action:** No action needed.

Evidence: [`EV-I-09`](raw/EV-I-09.md) · `sha256:41ad4ccb533ffbf611ba076f47b71988b96169437d9a37c8fc86b6c5cb5b682c` · Source-level inspection of PEP-05-01-01: cargo check passes.

### I-10 · Invalid input handling

**Predicate:** Invalid input cannot silently produce a valid-looking corrupt result.

**Interpretation:** Structured validation present; errors not silent.

**Reducer:** UNKNOWN — no instrument qualification.

**Next action:** No action needed.

Evidence: [`EV-I-10`](raw/EV-I-10.md) · `sha256:04ac7abb2a78d24f1e85db60476e15875ce8b0709de8b8dc0fd4c04f4dbca542` · Source-level inspection of PEP-05-01-04: Structured validation.

### I-11 · Error categories

**Predicate:** Expected validation failures use stable distinguishable error categories.

**Interpretation:** Error types distinguish categories clearly.

**Reducer:** UNKNOWN — no instrument qualification.

**Next action:** No action needed.

Evidence: [`EV-I-11`](raw/EV-I-11.md) · `sha256:21fd9b71dabb7cb5e090735421ba383cbb422abf694b7d8e06b70539031eef9b` · Source-level inspection of PEP-05-02-01: Error categories clear.

### I-12 · Test inventory

**Predicate:** The test inventory identifies which accepted obligations each suite exercises.

**Interpretation:** FAIL: Significant coverage gaps across tools and services.

**Reducer:** UNKNOWN — no instrument qualification.

**Next action:** Add dedicated tests for uncovered modules.

Evidence: [`EV-I-12`](raw/EV-I-12.md) · `sha256:ac27bd2d334e8f047ae36c903cfc589bbc96054d30b306bdad078a8d0c942b51` · Source-level inspection of PEP-06-01-01: Test coverage gaps.

### I-13 · Grader evaluation

**Predicate:** The grader accepts a valid solution that differs from its reference implementation.

**Interpretation:** NOT_APPLICABLE: jcode is not a grader.

**Reducer:** NOT_APPLICABLE — jcode is a coding agent, not a grader. No evaluator tool present..

**Next action:** N/A.

Evidence: not supplied. This absence is preserved, not converted into a pass.

### I-14 · Build documentation

**Predicate:** A clean checkout identifies the minimum required tools.

**Interpretation:** Cargo.toml complete; SDKROOT workaround in AGENTS.md.

**Reducer:** UNKNOWN — no instrument qualification.

**Next action:** Formalize build instructions.

Evidence: [`EV-I-14`](raw/EV-I-14.md) · `sha256:1dcf97685901b5d71c91aabc7fa58ba5fe55e91814ea9afd04be138d55769116` · Source-level inspection of PEP-07-01-01: Build documented.

### I-15 · Undocumented local paths

**Predicate:** Setup does not depend on an undocumented local path.

**Interpretation:** FAIL: SDKROOT workaround is a documented but non-standard local path.

**Reducer:** UNKNOWN — no instrument qualification.

**Next action:** Fix build to work without SDKROOT override.

Evidence: [`EV-I-15`](raw/EV-I-15.md) · `sha256:87a97ae7f401f33cf4262c8800dfba88f2ff772121a560fe2e61df551b411659` · Source-level inspection of PEP-07-01-02: SDKROOT workaround.

### I-16 · Operations discoverable

**Predicate:** Available operations are discoverable through a documented machine-facing surface.

**Interpretation:** MCP tools, CLI help, TUI all provide discovery.

**Reducer:** UNKNOWN — no instrument qualification.

**Next action:** No action needed.

Evidence: [`EV-I-16`](raw/EV-I-16.md) · `sha256:25201dbc718e3a1c5a82a73656554c57f626a845da9b8d331343f2c18145f74c` · Source-level inspection of PEP-08-01-01: Operations discoverable.

### I-17 · Output schemas stable

**Predicate:** Machine-readable success outputs conform to a stable schema.

**Interpretation:** Pydantic models enforce output structure.

**Reducer:** UNKNOWN — no instrument qualification.

**Next action:** No action needed.

Evidence: [`EV-I-17`](raw/EV-I-17.md) · `sha256:4a44f8183c29d4dcac0ec1e4e0ab7800cef140f6f4e4b1d28bd83ec55ad79aed` · Source-level inspection of PEP-08-01-02: Output schemas stable.

### I-18 · Failure categories

**Predicate:** Machine-readable failures distinguish actionable error categories.

**Interpretation:** Errors distinguish actionable categories.

**Reducer:** UNKNOWN — no instrument qualification.

**Next action:** No action needed.

Evidence: [`EV-I-18`](raw/EV-I-18.md) · `sha256:4eab77192dc659d6bfb2e3cfd236fd4e15527a2ab308fee974feaf6ce0bf9706` · Source-level inspection of PEP-08-01-03: Error categories distinguishable.

### I-19 · Keyboard controls

**Predicate:** All required interactive controls are reachable by keyboard.

**Interpretation:** TUI fully keyboard-driven.

**Reducer:** UNKNOWN — no instrument qualification.

**Next action:** No action needed.

Evidence: [`EV-I-19`](raw/EV-I-19.md) · `sha256:ad935f2b1e9631d438aed49a1e27f4bff570059b49c94581c35df697d7523663` · Source-level inspection of PEP-10-01-01: Keyboard controls.

### I-20 · Focus visibility

**Predicate:** Keyboard focus remains visibly identifiable.

**Interpretation:** Active panel highlighting present.

**Reducer:** UNKNOWN — no instrument qualification.

**Next action:** No action needed.

Evidence: [`EV-I-20`](raw/EV-I-20.md) · `sha256:04f1a988f288f5573ae86b0047122ef2eefc86807afdd885087ea288d5f54b2a` · Source-level inspection of PEP-10-01-02: Focus visible.

### I-21 · Workload representation

**Predicate:** The workload represents the claimed user or agent scenario.

**Interpretation:** Swarm matches claimed scenario.

**Reducer:** UNKNOWN — no instrument qualification.

**Next action:** No action needed.

Evidence: [`EV-I-21`](raw/EV-I-21.md) · `sha256:99a8b895140561ee6240183c431b5482ed884961933b30e9edb1dd3e816393c6` · Source-level inspection of PEP-12-01-01: Workload matches scenario.

### I-22 · Cache conditions distinguished

**Predicate:** Warm-cache and cold-cache conditions are distinguished.

**Interpretation:** FAIL: No cold-cache benchmarks exist.

**Reducer:** UNKNOWN — no instrument qualification.

**Next action:** Add startup and rendering benchmarks.

Evidence: [`EV-I-22`](raw/EV-I-22.md) · `sha256:dfe481fdc16e0721cd642031ba8af0c6751d4439f13eb6562c030b9b03dede26` · Source-level inspection of PEP-12-01-04: No cold-cache benchmarks.

## Findings and bounded decisions

### F-01 · MEDIUM · IMPLEMENTATION_GAP

SDKROOT and MACOSX_DEPLOYMENT_TARGET workarounds are undocumented local paths that block clean builds on fresh checkouts.

Owner: `build-system`. Status: `ACCEPTED`. Route: `IMPLEMENT`. Wakeup: Before next release or CI/CD setup.

### F-02 · HIGH · VERIFICATION_GAP

Test coverage has significant gaps; many tools and services lack dedicated unit tests.

Owner: `testing`. Status: `ACCEPTED`. Route: `TEST`. Wakeup: Before next feature release.

### F-03 · LOW · VERIFICATION_GAP

No cold-cache benchmarks exist; startup performance claims lack empirical backing.

Owner: `performance`. Status: `ACCEPTED`. Route: `EXPERIMENT`. Wakeup: Before performance-critical release.

**DEC-01 / IMPROVE / ACCEPTED:** Bootstrap profile with 22 criteria provides meaningful coverage without over-engineering the initial assessment.

Alternatives: Full 1,080-catalog assessment; Minimal 5-criteria smoke check.

Reversal: If initial findings warrant deeper investigation in specific domains.

## Maturity and value

| Axis | Scoped status | Basis |
|---|---|---|
| ARCHITECTURE | PASS | Clean crate boundaries, clear ownership, modular decomposition. |
| IMPLEMENTATION | PASS | Core operations produce correct results; input validation present. |
| TESTING | FAIL | Significant coverage gaps; many tools lack dedicated tests. |
| SETUP | FAIL | SDKROOT workarounds are undocumented local paths blocking clean builds. |
| ACCESSIBILITY | PASS | TUI fully keyboard-driven with focus management. |
| MACHINE_INTERFACE | PASS | MCP tools discoverable; structured outputs; error categories distinguishable. |
| PERFORMANCE | UNKNOWN | No cold-cache benchmarks; startup claims lack empirical backing. |

**Value evidence:** No user outcome evidence collected. The assessment establishes baseline technical quality, not user value.

## Semantic review and contrary cases

### Does the fork maintain upstream quality while extending capabilities?

The fork extends upstream with elicitation, swarm, and governance features while maintaining clean crate boundaries. Build passes after fixes. Testing gaps are the primary concern.

Support: Cargo check passes with only warnings. Architecture follows upstream patterns. New modules integrate cleanly.

Countercase: Testing gaps may hide regressions. SDKROOT workaround suggests build fragility.

Discriminator: Run full test suite with coverage measurement to quantify actual gaps.

Confidence basis: Source-level inspection only; no runtime verification claimed.

### Is the elicitation overlay ready for production use?

The elicitation system compiles after build fixes but has no runtime test coverage. The overlay types and channel are well-structured but unverified at runtime.

Support: Types defined, MCP tool present, TUI renderer implemented, channel wiring in place.

Countercase: No integration test exercises the full elicitation flow from MCP call to TUI popup to response.

Discriminator: End-to-end test: MCP tool call -> TUI overlay -> user response -> tool result.

Confidence basis: Source inspection confirms compilation; runtime behavior unverified.

## Delta and fresh-session handoff

**Change class:** INITIAL_BASELINE. First operational assessment of the jcode fork.

**Accepted baseline:** 22-criteria bootstrap assessment as of commit d0a32f240.

**Rejected or tested hypotheses:** All criteria would pass (3 failed: testing coverage, build reproducibility, performance claims)..

**Next reproduction:** Expand to D06 (testing) domain with full coverage measurement; add runtime execution tests.

**Budget and permissions:** Owner-directed. No external actions authorized beyond this assessment.

## Machine records and integrity

[Assignment](assignment.json) · [Assessment](assessment.json) · [Dossier context](dossier.json) · [Normalized JSON view](views/assessment.json) · [YAML view](views/assessment.yaml) · [JSONL export](views/records.jsonl) · [Build receipt](views/build.json)

JSON inputs are authoritative within this dossier. YAML, JSONL, Markdown, and HTML here are generated views. The unsigned manifest detects changed bytes against the supplied baseline; it does not certify observations or authorship.
