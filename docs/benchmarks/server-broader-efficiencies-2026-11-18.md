# Broader server efficiency acceptance, 2026-11-18

## Accepted changes

- `65e99c34c`: Gemini serializes exact legacy request bytes once and shares them across retries. Optimized alternating thread CPU improves 47–78% on retries, allocated bytes improve 43–70%. Single-attempt allocations remain unchanged. Frozen wire/header checks and 52 relevant worker tests passed.
- `96df37f2b`, `c5d5df618`: image preflight uses its already decoded bytes. Allocation calls fall from two to one, requested bytes and actual thread CPU approximately halve. Image fidelity, dimensions, limits and error behavior are unchanged. No-image work stays unchanged. See `image-preflight-allocation-2026-11-18.md`.
- `bbd9a2d66`: two eager mobile daemon loops share an identical default HTTP client. This removes duplicate certificate/client construction, not useful initialization. Optimized construction CPU falls 47.4%, cumulative allocation by 4.64 MiB. Warmed request allocation is unchanged. Exact auth/cookie/timeout/wire/pooling checks passed. This is not a whole-daemon CPU or retained-RAM percentage. See `startup-mobile-http-2026-11-18.md`.

## Integrated checks completed before final activation

Hosted combined check for base and Gemini tests, Gemini library tests, and strict production-library lint passed. A later combined hosted image/mobile integration run (`20261118-055721218-2703104`) passed 19 tests, with three ignored probes. Optimized performance probes were run separately as documented in their reports.

An isolated normal optimized executable, `startup-release-control-20261118-jcode`, passed eight sessions and two simultaneous-request waves with 1 MiB seeded tool results: all 16 exact streamed answers and persisted answers, full prior history, and session-owner release checks. Fixture: `broader-integrated-release-20261118`; dedicated home, TMPDIR, test flag, socket and loopback provider. No paid inference or normal session storage was used. This validates the pinned artifact, not yet the long-lived shared daemon.

The existing full-package integration scanner `every_provider_sends_clean_schemas` still expects an Anthropic dialect call in an old source file. That unrelated preexisting failure is not counted as passing. Existing unrelated base test-only strict-lint diagnostics are not suppressed.

## Final activation and stopping gate

1. Concurrent-peak profiling found substantial serialized JSON capacity slack. Probes `a532814cc` and `b36c547da` tested ordinary shrinking, 1.5x buffer growth, and conditional large-only shrinking with exact bytes/error/boundary checks. All were rejected as pure efficiencies: unconditional shrink added 46–49% tiny-body CPU, growth regressed CPU/allocation, and conditional shrink added ~2.8% tiny-body CPU plus one realloc for each triggered body. Large native-shaped retained capacity fell by about 8 MiB, but that is a tradeoff, not an accepted fix. Production serialization remains unchanged. See `examples/wire_capacity_probe/README.md` and raw results.
2. Coordinated hosted build of **b36c547da** succeeded. Its freshly rebuilt selfdev binary passed **16 sessions x two simultaneous-request waves**, verifying all **32 exact streamed and persisted answers**, complete prior history and owner release after disconnect. Fixture: `broader-final-selfdev-20261118`. This is a correctness/ownership test, not a before/after CPU or RSS comparison. The shared daemon was then activated. `selfdev status` confirms running/current/shared **b36c547da**, and `server:info` independently returns that hash.
3. The explicit deeper gates produced additional accepted fixes, then rejected remaining measured strategies on CPU/allocation evidence. No further validated pure improvement was identified among the examined owners. This is a scoped stopping point, not proof of global exhaustion, leak freedom or a lower total RSS. A concrete new profile finding should reopen investigation automatically. The bounded longevity results and startup controls retain their documented limits.

Combined hosted provider-core regression suite also passed **210 tests**, with four ignored probes (`20261118-060438694-2722052`). All implementation workers were awaited and stopped. No experimental parser profile, production cache eviction, transcript reduction or concurrency reduction was activated.

Fresh daemon attribution and rejected parser-profile experiment are in `server-fresh-daemon-memory-2026-11-18.md`. Long-lived and 32-session peak attribution are in `server-longevity-concurrency-2026-11-18.md`.
