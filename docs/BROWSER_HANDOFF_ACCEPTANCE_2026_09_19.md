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

Additional acceptance on the real Rust and Python documentation sites confirmed
automatic handoff selection in **4/4 ordinary fresh sessions** and correct final
page state and answers in **8/8 total sessions**. Python handoff performed actual
navigation in all three ordinary sessions. One eligible public-site timing pair
was **1.472x faster** with handoff. Two other Python controls violated the direct-only
instruction and are excluded from speed comparisons, not silently discarded.

## Measured workflow

- Parent model: `gpt-6-astra`, native OpenAI provider, identical in both arms.
- Handoff provider: explicitly `openrouter` using existing local BYOK credentials.
- Exact binary: `target/selfdev/jcode`, built via coordinated self-dev build.
- SHA-256: `eae091b252d546cca723a465acfb5e3085bd33eea428642c27513cda5f27c63f`.
- Built version: `50c4533fb-dirty-8dce190e3d68`. Its Rust changes were subsequently
  committed as `831171a47` without changing the measured Rust source.
- Immutable artifact: `~/.jcode/builds/versions/50c4533fb-dirty-8dce190e3d68/jcode`.
  A later confidence audit reconfirmed its SHA-256 above. Other concurrent work
  has since replaced `target/selfdev/jcode`, so use the immutable artifact when
  reproducing these particular measurements.
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

## Real public-site acceptance

These checks used the same exact binary, parent model, BYOK route, private daemon
and fresh CLI session isolation described above. They used real public pages,
not a localhost fixture, copied content, or mocked browser. An owned disposable
tab was reset before each trial. A separate browser DOM read after each CLI exit
checked the final URL, target heading visibility and actual page text. Exact
quoted text was compared with that independently read public page. Timings include
all initial handbacks and recovery actions.

### Rust book: safe recovery, not a productive-handoff speed claim

The ordinary prompt asked to open `https://doc.rust-lang.org/book/`, find the
References and Borrowing chapter and its Rules of References section, quote its
two rules, and leave that section visible. The direct prompt added the prohibition
on handoff. Both fresh sessions completed correctly:

- Ordinary: 14.753 seconds, five browser calls, automatically attempted handoff.
- Direct: 19.318 seconds, six browser calls, no handoff.
- Both quoted both rules exactly and left the correct section visible.

Neither handoff attempt executed a browser action. The blank-page attempt had no
content-script receiver. The attempt on the book returned uncertainty (confidence
0.670 below the unchanged 0.8 threshold). The parent recovered using direct
navigation. This validates automatic selection, conservative handback, recovery
and actual user-visible completion, but **does not demonstrate a productive
handoff speed advantage**. The elapsed difference is not counted as such.

### Python documentation: productive handoff and control compliance

The ordinary prompt asked to open `https://docs.python.org/3/`, navigate into the
Tutorial and then Whetting Your Appetite, quote the chapter's first sentence,
and leave the chapter open. It did not mention handoff. The direct prompt added
only the prohibition on handoff. Three pairs ran in alternating order:

| Pair | Default (s) | Direct-requested (s) | Actual handoff actions in default | Timing eligibility |
| --- | ---: | ---: | ---: | --- |
| 1 | 16.552 | 22.022 | 1 | Excluded: control attempted handoff |
| 2 | 17.069 | 9.536 | 2 | Excluded: control executed handoff |
| 3 | 12.203 | 17.968 | 2 | Eligible: control used only direct actions |

All six sessions returned chapter **1. Whetting Your Appetite**, quoted the full
first sentence correctly, and left the chapter heading visible at
`/tutorial/appetite.html`. The independently checked sentence was:

> If you do much work on computers, eventually you find that there’s some task you’d like to automate.

All three ordinary sessions chose handoff without being prompted to do so and
executed one or two navigation clicks through Jev. Pair 2's default session
resumed handoff and received `done`. Pair 1 recovered after one click using direct
actions. Pair 3 reached the target with two Jev clicks and used the handback's
page observation to provide the correct final answer. Handoff need not declare
`done` to be productive, but its actual actions and final state must be verified.

Only pair 3 satisfies both productive-default and no-handoff-control criteria:
**1.472x speedup, 32.1% lower elapsed time**, with four versus eight parent browser
calls. This is one eligible public-site pair, not a statistically established or
universal speedup. The two rejected controls remain in the evidence. They expose
a parent instruction-following limitation rather than proving direct-action
performance. These public-site results also expose avoidable initial blank-page
handoffs and the continuing need for safe parent recovery on complex pages.

Local raw evidence, including exact prompts, complete NDJSON tool results,
independent public-page snapshots, answers and binary hashes, is retained under:

- `$JCODE_SCRATCH_DIR/browser-handoff-public-rust/`
- `$JCODE_SCRATCH_DIR/browser-handoff-public-python/`

Real-site acceptance therefore extends the fixture result to actual user-facing
CLI/browser workflows, while retaining the subscription and activation blockers
below. No BYOK result is presented as subscriber acceptance.

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
- Confidence audit independently reran the backend suite: 324 passed, zero
  failed or skipped. Original coordinated logs reconfirmed 32 shared Jev and 52
  browser tests passed, with their live tests explicitly ignored, and the build
  completed with exit zero. These results establish the recorded source/build,
  not later unrelated edits in the shared working tree.
- Benchmark offline self-tests: passed.
- The current local build channel contains the new binary. The shared daemon
  remained on `b23f61316` during validation. Activation was deferred because an
  unrelated session was still processing. A normal session attaching to that
  old daemon does not yet use this change. Coordinate shared-server activation
  at an idle boundary before relying on it outside isolated tests.
