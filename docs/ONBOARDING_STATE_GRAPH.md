# Robust Onboarding: An Explicit State-Space Graph + Privacy-Preserving Trace Telemetry

Status: partially implemented (steps 1, 2, 5, 6 landed; see §5)
Owner: onboarding
Related code: `crates/jcode-tui/src/tui/app/onboarding_flow.rs`,
`onboarding_flow_control.rs`, `onboarding_graph.rs`, `onboarding_repair.rs`,
`onboarding_sim.rs`, `crates/jcode-tui/src/tui/app/tests/onboarding_eval.rs`,
`crates/jcode-base/src/auth/{env_facts,login_diagnostics,refresh_state,status_types}.rs`,
`crates/jcode-telemetry-core/src/{lib,onboarding_trace}.rs`

---

## 1. Why onboarding keeps breaking

Onboarding is not one flow. It is a **product of independent state spaces** that we
currently model only partially and in three different places:

| Axis | Values (roughly) | Where it lives today |
| --- | --- | --- |
| UI phase | `Login`, `LoginOpenAi`, `ModelSelect`, `ContinuePrompt`, `StartChoice`, `Suggestions`, `Done` | `OnboardingPhase` |
| Credential state, per provider | absent / present / verified / expired / **permanently rejected** | scattered: `auth-refresh-state.json`, `AuthStatus`, ad-hoc strings |
| Environment capability | tty? browser? bindable port? writable config dir? network? clock sane? keyring? | discovered *by failing*, then string-matched in `classify_auth_failure_message` |
| Import candidates | 5 external CLIs x present/absent/importable/stale | `ImportReview` |
| Transport mode | local / client-server / remote / sandbox | implicit |

The bug in today's log is exactly a cross-axis bug: OpenAI's refresh token was
**permanently invalidated** (`refresh_token_invalidated`, Jul 31 16:54 last success),
but the *credential axis* has no terminal `Rejected` state for OpenAI, so the catalog
sweep force-refreshed a dead token every ~15 minutes for two days. Claude *does* have
that state (`rejected_refresh_fingerprint` in `auth-refresh-state.json`) because someone
hit the bug there once and patched it locally. That is the tell: **we are patching cells
of a matrix we have never written down.**

Second tell from the same log: the UI said `GitHub Copilot - login expired` while every
`auth_status_check_fast` line said `copilot=not_configured`. Two code paths derive a
user-facing label from different notions of "credential state". A single enum makes that
class of bug unrepresentable.

So: the goal is not "fix onboarding". It is **make the state space explicit, make every
state reachable in tests, and make real-world traversals observable.**

---

## 2. The model

### 2.1 A node is a (phase, facts) pair, not just a phase

```rust
/// Everything the transition function is allowed to read. Pure data, cheap to
/// clone, cheap to construct in tests, and serializable so a real user's trace
/// can be replayed offline.
pub struct OnboardingWorld {
    pub env: EnvFacts,                     // capability probe results
    pub creds: BTreeMap<ProviderId, CredState>,
    pub imports: ImportFacts,
    pub transport: Transport,
    pub consent: TelemetryLevel,
}

pub struct OnboardingNode {
    pub phase: Phase,
    pub world: OnboardingWorld,
}
```

Transitions are a **pure function**:

```rust
fn step(node: &OnboardingNode, ev: Event) -> Transition
// Transition { next: Phase, effects: Vec<Effect>, edge: EdgeId }
```

`Effect` is a *description* of side effects (`OpenBrowser`, `BindCallbackPort(u16)`,
`WriteCreds`, `SpawnValidationPing`), never the side effect itself. That single
change is what makes the whole thing testable: the test harness executes effects
against a simulated world, production executes them against the real one, and
**both traverse identical edges**. `onboarding_sim.rs` today re-seeds phases by hand
and therefore can drift from the live flow; under this model the sim is just a
different `Effect` interpreter.

### 2.2 Credential state gets a real lifecycle

This is the highest-value single change and it directly fixes today's bug.

```
        ┌──────────────── refresh ok ────────────────┐
        v                                            │
   Absent ──login──> Present ──verify ok──> Verified ─┴─ expiry ─> Stale
      ^                  │                     │                    │
      │              verify fail          server 401             refresh
      │                  v                     v                    │
      └──── logout ── Unusable(reason) <── Rejected(fingerprint) <───┘   [TERMINAL]
```

Rules that fall out for free:

- `Rejected` is **terminal for that credential fingerprint**. No background sweep,
  catalog refresh, or retry may attempt it again. Only a *new* fingerprint (a real
  re-login) clears it. Generalize Claude's `rejected_refresh_fingerprint` to every
  provider, keyed by `sha256(refresh_token)[..8]` so we never store the token.
- The UI label is a `match` on this enum, one function, one place. `not_configured`
  can no longer render as "login expired".
- Fallback ranking (`Ctrl+Y` to Gemini) becomes a sort over `CredState`, not a pile
  of conditionals.
- `provider bootstrap` gets a precondition: `debug_assert!(!matches!(state, Rejected(_)))`.

### 2.3 Environment capabilities are probed, not discovered by failing

`classify_auth_failure_message` is a 50-line string matcher over English error text.
It works, but it runs *after* we have already burned the user's first 90 seconds on a
flow that could never succeed. Invert it:

```rust
pub struct EnvFacts {
    pub tty: Tri,               // interactive stdin/stdout
    pub browser: Tri,           // xdg-open / open / cmd exists and a display exists
    pub loopback_bind: Tri,     // can we bind 127.0.0.1:0 and a fixed callback port
    pub config_writable: Tri,   // ~/.jcode writable, not read-only FS, not full
    pub network: Tri,           // provider host reachable, TLS ok, no captive portal
    pub clock_skew_ok: Tri,     // |now - server Date header| < 5 min (JWT killer)
    pub keyring: Tri,
    pub proxy: Tri,
    pub container: Tri,         // docker/WSL/ssh/codespace -> browser flows unreliable
}
```

`Tri = Yes | No | Unknown`, probed concurrently in <200ms at first-run, cached per
boot. Then **method selection is a lookup, not a hope**:

| Facts | Chosen auth method |
| --- | --- |
| browser=Yes, loopback=Yes | OAuth loopback (best) |
| browser=Yes, loopback=No | OAuth with paste-back callback URL |
| browser=No, tty=Yes | Device code flow / `--print-auth-url` |
| tty=No | API key from env/stdin, otherwise fail *fast* with a copyable command |
| config_writable=No | Refuse to start login; explain the real problem first |
| clock_skew_ok=No | Fix-the-clock screen; do not attempt OAuth at all |

Every one of these is a documented node with a documented recovery edge. Today most
of them are a generic error toast plus `onboarding_repair.rs`'s "ask another AI agent
to fix it", which is a great last resort and a bad first resort.

### 2.4 Invariants the graph must satisfy (checked in CI, not by review)

These are the payoff. Once the graph is data, you can assert over it:

1. **No dead ends.** Every non-terminal node has ≥1 outgoing edge reachable by a key
   the user can actually press, and that edge is named on screen.
2. **Every failure node has a recovery edge** that is not "restart jcode".
3. **Bounded work.** `max steps-to-ready ≤ N` and `max keystrokes ≤ K` over all paths
   (Tier 1 of `onboarding_eval.rs` already counts this; the graph makes it exhaustive
   instead of authored-by-hand).
4. **Reachability.** Every node is reachable from `Boot` under *some* `EnvFacts`, and
   any node reachable under *no* `EnvFacts` is dead code and must be deleted.
5. **Escape hatch everywhere.** Every node accepts `Esc`/skip and lands in a usable
   app, possibly degraded. Nobody is ever trapped in first-run.
6. **Progress.** No cycle without a user-visible state change (kills retry loops like
   the one in today's log).
7. **Terminal-state respect.** No effect targets a provider in `Rejected`.

Enforcement: a `#[test]` that walks the graph exhaustively. The env fact space is
~3^9 but collapses hard under equivalence classes; even brute force at 20k nodes is
milliseconds. Plus a `proptest` model-based test that drives random event sequences
against the real `App` and asserts the invariants hold at every observed state, and
that the `App`'s state always equals the model's state (this is the anti-drift check
that `classify_phase_surface`'s wildcard-free `match` gestures at today, generalized).

---

## 3. Telemetry: traces of a graph, not logs of a program

The question was how to monitor this without touching credentials. The graph makes
that easy, because **the interesting signal is the shape of the traversal, and the
shape is a list of small integers.**

### 3.1 What we send

One event per session: an ordered trace.

```jsonc
{
  "event": "onboarding_trace",
  "schema_version": 3,
  "install_id": "<existing anonymous telemetry id>",
  "run_id": "<random per boot>",
  "env": {                      // Tri values only, no hostnames, no paths
    "tty": "yes", "browser": "no", "loopback_bind": "yes",
    "config_writable": "yes", "network": "yes", "clock_skew_ok": "yes",
    "container": "yes", "proxy": "unknown"
  },
  "steps": [
    { "node": "boot",              "edge": "probe_done",        "dt_ms": 180 },
    { "node": "login_pick",        "edge": "choose_openai",     "dt_ms": 4200 },
    { "node": "oauth_loopback",    "edge": "fail",              "dt_ms": 61000,
      "reason": "callback_timeout" },
    { "node": "recover_offer",     "edge": "device_code",       "dt_ms": 2100 },
    { "node": "device_code",       "edge": "ok",                "dt_ms": 15400 },
    { "node": "validate",          "edge": "ok",                "dt_ms": 900 },
    { "node": "ready",             "edge": null,                "dt_ms": 0 }
  ],
  "outcome": "ready",           // ready | abandoned | degraded | stuck
  "keystrokes": 7
}
```

Everything in `node`, `edge`, `reason`, and `outcome` comes from a **closed
`&'static str` vocabulary defined in the graph itself**. There is no path for
user data to enter these fields, because there is no free-text field. That is a
structural privacy guarantee, not a scrubbing policy. Contrast with the current
`auth_failure_reason`, which is right in spirit (`AuthFailureReason` is already a
closed enum) but is derived by string-matching an error message we do also log.

### 3.2 Privacy rules (enforced by types, then by test)

1. **Closed vocabulary only.** The trace event struct's string fields are
   `&'static str` sourced from `NodeId`/`EdgeId`/`ReasonId` enums. A test asserts
   every emitted value is a member of the registry.
2. **No secrets, ever.** Where identity matters (did the refresh token change?) send
   `sha256(secret)[..8]` and only ever compare it to a locally stored fingerprint.
   Salted per install so fingerprints are not cross-user joinable.
3. **No paths, hostnames, usernames, emails, org names, model IDs from private
   deployments.** Provider is an enum member (`openai`, `anthropic`, ...); an
   unrecognized custom provider reports as `custom`.
4. **Timings are bucketed** (`dt_ms` rounded to 100ms, capped at 300s) so latency
   patterns can't act as a behavioral fingerprint.
5. **Consent-tiered**, reusing the existing three-way `TelemetryLevel`:
   `Nothing` sends nothing; `NoContent` sends the full trace (it contains no content
   by construction); `Everything` additionally allows attaching the redacted error
   *string* for unclassified `reason: "unknown"` cases, which is how the taxonomy
   grows.
6. **k-anonymity on the aggregation side.** Any (env, trace-shape) cohort with fewer
   than k=20 installs is reported only as "rare". Rare-but-fatal shapes still surface
   as a count, without the env vector.
7. **Local-first and inspectable.** `jcode telemetry show-last-trace` prints the exact
   bytes we would send. `--dry-run` mode writes them to disk and sends nothing. If a
   user can read the whole payload in 20 lines, trust is cheap.
8. **Cap and drop.** Traces are bounded (say 64 steps); overflow reports
   `truncated: true`. No unbounded queues (already the case in telemetry-core).

### 3.3 What we learn, and the loop it closes

Because every trace is a path through a known graph, aggregation is trivial and the
questions answer themselves:

- **Edge failure rate**: `oauth_loopback --fail--> ...` at 30% in `container=yes`
  installs means we should never pick loopback in containers. That is one table entry.
- **Abandonment attribution**: which node was last before `outcome=abandoned`, split
  by env. Today we genuinely cannot answer this, which is why `onboarding_eval.rs`
  opens with "we cannot collect data from real users, so we score the artifact".
  The artifact score is a good proxy; it is not a funnel.
- **Unreachable-in-practice nodes**: authored screens nobody ever hits -> delete them.
- **Novel failures**: `reason: "unknown"` rate is the taxonomy's health metric. Drive
  it toward zero, one classifier at a time.
- **Regression alarms**: a release that moves `p50 steps-to-ready` or the
  `ready` rate outside an error budget is a release you roll back.
- **Replay**: a trace is a deterministic script. Feed a real user's failing trace into
  the sim harness and watch their exact screens, with zero access to their data. This
  is the single biggest debugging upgrade in the proposal.

---

## 4. Runtime robustness policies the graph makes expressible

Once states are explicit, the fixes for today's log are one-liners rather than
whack-a-mole:

- **Terminal-rejection guard** (fixes the 2-day retry loop): background sweeps filter
  providers by `CredState`, skipping `Rejected`. Applies to OpenAI, Copilot, Cursor,
  Gemini, all of them, because it is a property of the state, not the provider.
- **Circuit breaker per (provider, effect)**: exponential backoff with a cap, and a
  hard stop on terminal classifications. Telemetry-core already has a process breaker
  for permanent statuses; generalize the concept to auth.
- **Degraded-ready is a first-class outcome**: if *any* provider is `Verified`, the
  user reaches a working app and the broken provider becomes a dismissible task, not
  a blocking screen. Today's session did offer the Gemini fallback, which is the right
  instinct; make it the default path rather than a `Ctrl+Y` hint after a hard stop.
- **Idempotent, atomic credential writes**: write-temp + rename + fsync, with the
  `.bak` rotation that already exists, so a crash mid-login can never produce a
  half-written `openai-auth.json`.
- **Self-check on boot**: run the invariant checks against the *live* world and log
  (locally) any violated invariant. Cheap, and it catches drift in the field.

---

## 5. Implementation plan (incremental, no big bang)

The existing code is in decent shape; this is mostly consolidation.

1. **`CredState` enum + universal rejection fingerprints.** *Landed.*
   `auth::refresh_state::CredState` is the lifecycle in §2.2, and every OAuth
   provider now records refresh outcomes through `record_refresh_outcome`, so no
   provider can silently opt out of terminal rejection. `ensure_refresh_allowed`
   is the guard callers use before spending a round-trip.
2. **`EnvFacts` probe.** *Landed.* `auth::env_facts` probes tty, browser,
   loopback bind, writable config, container, and proxy in under a millisecond,
   with the §2.3 selection table tested exhaustively over the whole 3^5 fact
   space. It is wired into `auth::browser_suppressed`, so a machine that
   positively cannot use a browser skips straight to a device/paste flow instead
   of waiting out a callback timeout.
3. **Extract the transition table.** *Not started.* Move the logic in
   `onboarding_flow_control.rs` (1.7k lines) behind `step(node, ev) ->
   Transition`, keeping current behavior byte-identical; the golden tests in
   `onboarding_golden.rs` are the safety net.
4. **Effect interpreter split.** *Not started.* Live interpreter + sim
   interpreter; deletes the hand-seeded phase list in `onboarding_sim.rs`.
5. **Invariant tests.** *Landed.* `onboarding_graph.rs` declares the graph as
   data (including the `EnvBlocked`, `LoginFailed`, and `CredRejected` states the
   flow always had but never modelled) and `check_invariants` enforces the §2.4
   properties. Wired into `scripts/check_guardrails.sh`.
6. **Trace telemetry.** *Landed as a library.*
   `jcode_telemetry_core::onboarding_trace` records traversals with bucketed
   timings, a hard step cap, and a closed vocabulary enforced by a test that
   walks the serialized payload and rejects any free text. Not yet emitted from
   the live flow: that waits on step 3, which is what produces the edge events.
7. **Method selection from `EnvFacts`.** *Partially landed* via
   `browser_suppressed`; the full table drives only the browser/no-browser
   decision so far, not device-code vs paste-callback.
8. **Aggregation + k-anonymity** on the receiving side, plus a `steps-to-ready` /
   `ready-rate` dashboard with an error budget. *Not started.*

Rough ordering principle: every step is independently shippable and independently
valuable, and steps 1 and 5 alone would have prevented both bugs visible in the
log that prompted this document.

---

## 6. Risks

- **Over-abstraction.** A state machine framework that is harder to read than the
  conditionals it replaced is a net loss. Mitigation: the transition table must be
  readable as a table by a person who has never seen the code. If it isn't, stop.
- **Probe flakiness.** A wrong `browser=No` sends users down a worse path than
  failing forward would have. Mitigation: `Unknown` biases toward the optimistic path,
  and step 2 validates probes against reality before they gate anything.
- **Telemetry trust.** Any perception that we ship credentials or prompts is fatal and
  irreversible. Mitigation: closed vocabulary by construction, `show-last-trace`,
  default to `NoContent`, and document the exact schema publicly (this file).
