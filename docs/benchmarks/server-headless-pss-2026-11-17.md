# Single-server, single-headless-agent PSS, 2026-11-17

## Result

Commit `821194f84` avoids materializing the entire models.dev catalog as a JSON value tree. It borrows raw provider/model envelopes and deserializes only pricing fields. This removes a large transient allocation burst without disabling pricing discovery.

Across three paired runs, server PSS after two tool turns and 15 seconds idle fell from a median **171.56 MiB to 109.23 MiB**, a **62.33 MiB (36.3%) reduction**. These samples precede explicit allocator purging.

**This is a refresh-retention improvement, not a demonstrated reduction of the long-term idle floor.** A separate zero-turn, 130-second idle control allows the existing 120-second retained-heap watchdog to run. Baseline converged to 97.57 MiB and optimized to 99.35 MiB without explicit purging. The baseline already eventually returns its retained pricing allocations to the OS.

## Method

Use `scripts/profile_server_memory.py` with isolated auth copies, JCODE_HOME, runtime directory, TMPDIR and server socket. Each run starts a fresh server with no TUI, creates exactly one headless agent, and samples only that server PID using Linux smaps_rollup. Full smaps, allocator statistics, session population and build metadata are retained. Credentials and private runtime directories are removed on exit.

The paired workload used model `gpt-6.1-sol`, two successful turns reading the first 20 lines of README.md through the read tool, then 15 seconds idle. No swarm was requested. Explicit allocator purge is a separate final stage and is not included in the headline result.

Both pinned binaries use the system allocator and the hosted selfdev build profile. They have separate executable inodes. Their approximately 80 MiB of charged file-backed pages make absolute PSS higher than a process sharing executable pages with other instances. Do not compare these absolute values directly with the earlier shared-binary probes. Unrelated concurrent source edits were preserved, so the builds are not a formally pricing-only source comparison. Heap attribution, the pricing-disabled control, repeated paired profiles, catalog equality and purge convergence support the pricing-retention diagnosis.

## Measurements

All values below are server-only PSS in KiB.

| Pair | Baseline session idle | Optimized session idle | Baseline after two turns + settle | Optimized after two turns + settle | Baseline after explicit purge | Optimized after explicit purge |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 167434 | 100618 | 172433 | 110072 | 104437 | 106041 |
| 2 | 168370 | 104881 | 175678 | 111850 | 107162 | 107362 |
| 3 | 171030 | 105442 | 178907 | 112610 | 108031 | 107138 |

At the settled two-turn stage, anonymous PSS was roughly 88–91 MiB before versus 25–26 MiB after. Live allocator bytes remained about 6 MiB. Retained allocator bytes fell from about 69–72 MiB to 6–8 MiB. Similar post-purge results confirm that the large difference is freed memory retained by the allocator, not live agent state.

Separate completed-refresh, zero-turn control:

| Stage | Baseline PSS KiB | Optimized PSS KiB |
| --- | ---: | ---: |
| Startup | 170770 | 101717 |
| One headless session idle | 175322 | 106054 |
| After 130 seconds idle, no explicit purge | 99908 | 101734 |
| After explicit purge | 99853 | 98170 |

These long controls use independent copies of the pinned binaries to avoid executable-page sharing with the repeated workload runs. Earlier long controls interrupted by a server reload are excluded.

## Attribution and correctness

A cumulative jemalloc allocation profile attributed 60.01 MiB to `model_pricing::parse_api_response`, 74.01 MiB to pricing refresh overall, out of 120.39 MiB sampled cumulative startup allocation. These are cumulative allocation estimates, not simultaneous live bytes or PSS. An earlier pricing-refresh-disabled control also removed the startup burst.

The replacement parser skips irrelevant metadata without building its JSON tree. It retains malformed-sibling tolerance, numeric pricing semantics and arbitrary future provider/model IDs. Comparing the baseline and optimized long-control catalogs found exact equality of all prices across **312 providers and 9,474 models**, excluding the refresh timestamp.

Validation: 13 Rust pricing tests, 9 Python profiler tests, hosted check/build and clippy with warnings denied passed. The rebuilt server was activated and its runtime git hash verified as `821194f84`.

## Reproduction

```bash
python3 scripts/profile_server_memory.py \
  --binary /absolute/path/to/jcode --cwd /home/jeremy/jcode \
  --model gpt-6.1-sol --turns 2 --settle-seconds 15 \
  --require-pricing-refresh \
  --prompt 'Use the read tool to read the first 20 lines of README.md, then reply with its title only. Do not use swarm or modify any files.' \
  --output /new/private/artifact-directory
```

For the watchdog control, use `--turns 0 --settle-seconds 130 --require-pricing-refresh`. For sampled heaps, build with jemalloc profiling and add `--heap-profile`; do not compare its absolute PSS directly with the system-allocator runs.

Local artifacts are under `/home/jeremy/.jcode/scratch/`: `server-pss-before`, `server-pss-after`, their `-2`/`-3` repetitions, and `server-pss-before-watchdog` / `server-pss-after-watchdog`. Sampled attribution is in `pricing-before-allocation-attribution.txt` and `headless-profile-vsie7v99`.

Pinned system binary SHA256:

- Before: `be033aeaedb6bd00b06eb5ff856dc2d8ec53cf24f191f12fc6ed2a648fe04647`
- After: `2ce1b2b88f799f343edae070ee65029a54ee994c2c3db12b3ea5639cb0240e99`

The remaining idle floor includes substantial file-backed executable pages. Further PSS work should measure representative optimized builds and attribute remaining anonymous mappings independently, rather than assuming this change saves another 62 MiB after the watchdog has already run.
