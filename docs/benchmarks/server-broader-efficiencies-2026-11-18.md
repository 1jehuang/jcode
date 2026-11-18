# Broader server efficiency acceptance, 2026-11-18

## Accepted changes

- `65e99c34c`: Gemini serializes exact legacy request bytes once and shares them across retries. Optimized alternating thread CPU improves 47–78% on retries, allocated bytes improve 43–70%. Single-attempt allocations remain unchanged. Frozen wire/header checks and 52 relevant worker tests passed.
- `96df37f2b`, `c5d5df618`: image preflight uses its already decoded bytes. Allocation calls fall from two to one, requested bytes and actual thread CPU approximately halve. Image fidelity, dimensions, limits and error behavior are unchanged. No-image work stays unchanged. See `image-preflight-allocation-2026-11-18.md`.
- `bbd9a2d66`: two eager mobile daemon loops share an identical default HTTP client. This removes duplicate certificate/client construction, not useful initialization. Optimized construction CPU falls 47.4%, cumulative allocation by 4.64 MiB. Warmed request allocation is unchanged. Exact auth/cookie/timeout/wire/pooling checks passed. This is not a whole-daemon CPU or retained-RAM percentage. See `startup-mobile-http-2026-11-18.md`.

## Integrated checks completed before final activation

Hosted combined check for base and Gemini tests, Gemini library tests, and strict production-library lint passed. A later combined hosted image/mobile integration run (`20261118-055721218-2703104`) passed 19 tests, with three ignored probes. Optimized performance probes were run separately as documented in their reports.

An isolated normal optimized executable, `startup-release-control-20261118-jcode`, passed eight sessions and two simultaneous-request waves with 1 MiB seeded tool results: all 16 exact streamed answers and persisted answers, full prior history, and session-owner release checks. Fixture: `broader-integrated-release-20261118`; dedicated home, TMPDIR, test flag, socket and loopback provider. No paid inference or normal session storage was used. This validates the pinned artifact, not yet the long-lived shared daemon.

The existing full-package integration scanner `every_provider_sends_clean_schemas` still expects an Anthropic dialect call in an old source file. That unrelated preexisting failure is not counted as passing. Existing unrelated base test-only strict-lint diagnostics are not suppressed.

## Explicit remaining gates

1. Concurrent-peak profiling found serialized JSON buffer capacity substantially exceeds wire length. A dedicated CPU/allocation/byte-equivalence experiment is in progress. Reject it if memory savings require repeatable CPU regression.
2. Finish coordinated build/activation of accepted source, verify current/shared daemon identity, and run its isolated representative fixture.
3. Challenge the stopping gate against concrete remaining owners. The bounded longevity results and fresh startup controls do not prove leak freedom, global optimization exhaustion, or a lower total RSS.

Fresh daemon attribution and rejected parser-profile experiment are in `server-fresh-daemon-memory-2026-11-18.md`. Long-lived and 32-session peak attribution are in `server-longevity-concurrency-2026-11-18.md`.
