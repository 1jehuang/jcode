# Browser handoff speed measurement, 2026-09-21

## Answer supported by the evidence

An archived three-pair navigation benchmark measured a **1.685x median paired
speedup**, equivalent to **40.65% less elapsed time** for the median pair.
Both arms completed correctly in all three trials. This is an older implementation,
not a verified speedup for the latest whole-task controller.

The current implementation could not be timed successfully against Jev because
its configured OpenRouter route returned HTTP 402 (credits/account spending limit
exhausted). A failed handoff is not a fast completion. No purchase was made.

## Re-audited archived comparison

Source: local `browser-handoff-benchmark-byok-v2` artifacts in Jcode's scratch
folder. The six original NDJSON transcripts were reparsed using the current
`benchmark_browser_handoff.py` extractor on 2026-09-21. Assertions confirmed:

- All six recorded independent final-DOM, fixture and returned-receipt checks passed.
- No direct-arm transcript executed handoff. Neither arm executed non-browser tools.
- Each handoff-arm run contained an initial zero-step handback, a parent `open`,
  then a successful handoff executing four navigation steps. The entire time,
  including recovery, remains in the measurement.
- No trace errors were present. This was natural-default routing, not the newer
  explicit-Jev experiment. It must not be pooled with that experiment.

| Pair | Handoff-assisted total | Direct total | Direct / handoff |
| --- | ---: | ---: | ---: |
| 1 | 16.661 s | 26.355 s | 1.582x |
| 2 | 14.450 s | 25.233 s | 1.746x |
| 3 | 14.350 s | 24.178 s | 1.685x |

Median elapsed time by arm was **14.450 s handoff-assisted** versus
**25.233 s direct**. The ratio of these two medians is not the median paired
ratio above. Parent browser tool calls were **4 versus 11** in every pair.
This is consistent with fewer parent-model/tool round trips, but is not a causal
latency breakdown or a measured model-call count.

Timing covers client launch through final response/process exit, excluding
isolated daemon startup. Parent model was `gpt-6-astra`, provider `openai`.
Jev used OpenRouter. Archived binary SHA-256:
`eae091b252d546cca723a465acfb5e3085bd33eea428642c27513cda5f27c63f`.

Only three pairs and one synthetic navigation task were tested. These results
are descriptive, not evidence of a universal speedup, a 2x guarantee, or current
search/form performance. Other archived public-site samples were excluded from
this claim because some direct arms called handoff or handoff made no progress.

Recomputed local audit: `browser-handoff-speed-20260921-audit.json` in scratch.

## Current implementation attempts and blocker

The existing paired harness self-test passed. A dedicated disposable local tab,
private sockets and isolated daemons were used. The selected immutable binary was
`1d8388635-dirty-e56b23724f90/jcode` (v0.86.17-dev). Its direct-arm disable guard
was present. No production code or prompts were tuned during measurement.

All attempted trials remain in separate scratch output folders:

| Output folder suffix | Attempts | Observed outcome |
| --- | ---: | --- |
| `browser-handoff-speed-20260921-discovery` | 18 | Isolated home had no usable parent credentials. All failed before browser actions. No eligible speed pairs. |
| `browser-handoff-speed-20260921-auth-smoke` | 2 | Existing OpenAI API route returned insufficient quota in both arms. No eligible speed pair. |
| `browser-handoff-speed-20260921-oauth-smoke` | 2 | Existing subscription parent worked. Jev returned HTTP 402 with zero steps. Direct completed correctly. No eligible speed pair. |

In the subscription smoke, direct took **38.535 s**, with the receipt visible at
**29.748 s**. The Jev arm returned failure after **14.744 s**. Those values must
**not** be divided and presented as a speedup because the Jev task did not finish.
No configured TypeSafe, AIMLAPI or Jcode subscription credential file was available
as an alternate Jev route. The OpenRouter key was used only through its intended
provider. No alternate account was silently charged.

The planned three-pair-per-task discovery and held-out comparison is therefore
blocked, not completed. Once Jev access is restored, run the documented harness
with identical frozen conditions in both arms and preserve all failures. Do not
combine those future results with the archived natural-default experiment.

See [benchmark methodology](../scripts/benchmark_browser_handoff.md) for exact
acceptance rules and telemetry limitations. Raw local transcripts are not copied
into this report, and no credentials are included.
