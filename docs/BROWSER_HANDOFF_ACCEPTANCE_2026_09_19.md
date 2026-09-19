# Browser handoff acceptance: 2026-09-19

## Outcome

The browser description and action schema explicitly default browser tasks to
Jev handoff. Three independently launched fresh Jcode sessions selected handoff
from ordinary navigation prompts that never mentioned handoff. All three
completed four navigation clicks through Jev and returned the fresh receipt.
Three paired direct-action sessions also completed correctly.

**This establishes the new CLI's default selection, browser correctness, and a
measured BYOK speed advantage on this workload. It does not establish live Jcode
subscription availability.**

## Measured workflow

- Parent model: `gpt-6-astra`, native OpenAI provider, identical in both arms.
- Handoff provider: explicitly `openrouter` using existing local BYOK credentials.
- Exact binary: `target/selfdev/jcode`, built via coordinated self-dev build.
- SHA-256: `eae091b252d546cca723a465acfb5e3085bd33eea428642c27513cda5f27c63f`.
- Built version: `50c4533fb-dirty-8dce190e3d68`. Its Rust changes were subsequently
  committed as `831171a47` without changing the measured Rust source.
- Each trial launched that exact binary as its own daemon with private socket
  and runtime directory, then a fresh `jcode run --ndjson` session in an empty
  workspace. It did not use the old shared daemon.
- Dedicated tab, fresh loopback URL and receipt per trial. Normal and direct
  order alternated. No measured trials were dropped or retried.
- Ordinary prompt asked to navigate guides to Browser guide, Navigation examples,
  then the navigation receipt, return that receipt, and leave the page open.
  The direct arm added only the requirement not to use handoff.
- Timing runs from CLI client launch to exit, including parent reasoning, tools,
  handbacks, and final response. Daemon startup is excluded.
- Correctness required server evidence, a document-lifecycle beacon, a separate
  scoped final DOM probe, and the correct fresh receipt in the final answer.

| Pair | Default handoff (s) | Direct actions (s) | Direct / handoff |
| --- | ---: | ---: | ---: |
| 1 | 16.661 | 26.355 | 1.582 |
| 2 | 14.450 | 25.233 | 1.746 |
| 3 | 14.350 | 24.178 | 1.685 |

- Default handoff selection: **3/3**.
- Correct, compliant completion: **6/6**.
- Median paired speedup: **1.685x**, about **41% lower elapsed time**.
- Each default trial used four parent browser calls: status, an initial handoff,
  open, then a successful handoff. The first handoff safely returned with low
  confidence because the starting tab was not yet on the requested URL. The
  second reported `done`, `decision_provider: openrouter`, and four executed
  clicks. Those initial handbacks are included in the measured times.
- Each direct trial used eleven parent browser calls.

This small, simple navigation workload does not establish a universal speedup,
complex-form performance, other parent-model behavior, or subscription latency.
An initial harness readiness failure produced no measured trials. It was fixed
by enabling debug control only on the private daemon and passing its socket to
the debug subcommand explicitly.

Local raw evidence is retained under
`$JCODE_SCRATCH_DIR/browser-handoff-benchmark-byok-v2/`, including per-trial prompts,
NDJSON transcripts, independent result records, binary metadata and summary.
See `scripts/benchmark_browser_handoff.md` for repeatable invocation.

## Subscription implementation and blockers

The browser uses the shared Jev client with a separate browser purpose and
`JCODE_BROWSER_JEV_PROVIDER`. Auto selection prefers Jcode credentials, checks
live `/v1/me` capability `browser_jev`, then uses bounded typed choice requests at
`/v1/decisions`. It never silently spends a BYOK balance after an entitlement or
billing failure. Memory provider selection and `memory_jev` gating remain separate.

Backend commit `61c6572` in `solosystems-backend` adds strict browser choice and
response validation, capability advertisement, and existing paid-subscription
gating with a shared account-wide quota (60 attempts/minute, 1,000/day by default).
Memory and browser calls cannot bypass that quota by switching purpose.

Live subscription verification was attempted with
`JCODE_BROWSER_JEV_PROVIDER=jcode` and the ignored
`live_subscription_jev_decision_smoke` test. It stopped at missing Jcode credentials,
without falling back or transmitting fixture data. A read-only deployed secret
name check separately confirmed that the gateway has no `OPENROUTER_API_KEY`.
No personal key was copied to the gateway, no subscription was created, and no
billing settings were altered. The backend change has **not been deployed**.

To finish subscription acceptance:

1. Sign in to an eligible account with `jcode account login`.
2. Provision an explicitly approved dedicated, hard-spend-capped gateway key.
3. Deploy the reviewed backend change and verify `browser_jev: true` for the
   entitled account, while denied accounts remain denied.
4. Run the subscription-only smoke test, then the fresh-session benchmark with
   `JCODE_BROWSER_JEV_PROVIDER=jcode` and expected provider `jcode`.

## Other verification and activation

- Shared Jev suite: 32 passed, 1 live test ignored.
- Browser suite: 52 passed, 5 live tests ignored.
- Real BYOK typed-decision smoke: passed.
- Backend full suite: 324 passed.
- Benchmark offline self-tests: passed.
- The current local build channel contains the new binary. The shared daemon
  remained on `b23f61316` during validation. Activation was deferred because an
  unrelated session was still processing. A normal session attaching to that
  old daemon does not yet use this change. Coordinate shared-server activation
  at an idle boundary before relying on it outside isolated tests.
