# Jcode vs Maki: local TUI performance and resources

Measured October 27, 2026 UTC, on the user's Dell XPS 14, Intel Core Ultra X9 388H, 62.3 GiB RAM. Linux x86_64, kernel 7.2.11-arch1-1, 16 logical CPUs. Initial load averages: 2.63 / 3.18 / 3.82. This was an active development laptop, not a dedicated benchmark machine.

## Scope and conclusion

**This measures local TUI overhead, not coding quality, model latency, token efficiency, or tool throughput.** No prompts were submitted and no valid model credentials were supplied. Different selected model names therefore do not contribute inference work to these measurements.

- Maki has a substantially smaller one-session footprint: **67.9 MiB vs Jcode's 218.8 MiB PSS**, including Jcode's server.
- Jcode scales more economically across these idle sessions. At ten sessions it uses **327.0 MiB vs Maki's 394.9 MiB PSS**, about 17% less. The measured PSS crossover lies somewhere between five and ten sessions, not at an established exact threshold.
- First-session editable startup was **71 ms Jcode vs 138 ms Maki** in the three one-session repetitions. Both were responsive to typing, with typical echo times of a few milliseconds.
- **Do not interpret Maki's animated welcome-screen CPU as steady-state chat CPU.** Disabling its splash animation reduced measured CPU substantially. The resulting CPU figures are much closer to Jcode's, and Jcode's ten-session samples were noisy.
- This compares an installed **Jcode development binary** to a **Maki release binary**. It is not an optimized-release-versus-optimized-release comparison.

## Binaries

Executables were resolved once before each suite. Every recorded running process executable matched the corresponding pinned binary, including Jcode's dedicated server.

| | Jcode | Maki |
|---|---|---|
| Version | v0.99.252-dev, commit `8e1417e79` | 0.6.5 |
| File size | 325,992,785 bytes, 310.9 MiB | 116,670,208 bytes, 111.3 MiB |
| SHA-256 | `a54f6d87d5f739ab654a290a1fa26c0b458271bf964430d00cec06c7f72ea9b9` | `9d6c1a61ac6c706c6d4b45b72e7b032b10f52b89f0cc3c019d24865e77f759da` |

File size is not resident memory and includes whatever symbols and embedded assets each distribution ships.

## Method

The main suite contains **18 runs**: two tools, three session counts, three repetitions. Run order was shuffled with seed 42. A further **nine runs** measure Maki with splash animation disabled, again at all three session counts with three repetitions each.

Each run uses:

- A fresh isolated HOME, XDG directories, and empty workspace. No real project is opened or modified.
- A dummy Anthropic API key. No real auth or user configuration is copied.
- A 120-column by 40-row pseudoterminal per client, with kernel input echo disabled. Terminal capability requests are answered.
- Actual rendered chat-prompt detection followed by application-rendered input echo. Initial status text alone does not qualify as input-ready.
- Two seconds of settling after all clients launch, then approximately five seconds of idle observation. Process accounting extends the actual interval to roughly 5.5 to 6.3 seconds.
- `/proc` PSS, RSS, private memory, and CPU accounting for the complete measured process tree. Jcode totals include one dedicated server plus all clients. Maki totals include all independent instances.
- Twenty additional short input strings per run, spread round-robin across the terminals. No Enter key is sent.
- Owned process groups terminated after measurement. The user's active daemon is not reused or restarted.

Jcode: `--no-update --no-selfdev`, memory disabled through `JCODE_MEMORY_ENABLED=0`, isolated socket, and returning-user markers to skip onboarding. Maki: `--no-plugins --no-commands`, keeping builtin plugins. The splash-disabled control replaces `--no-plugins` with an isolated global `init.lua` containing only:

```lua
maki.setup({ ui = { splash_animation = false } })
```

All other user plugins remain absent because the control uses a fresh HOME. The control's boolean `maki_no_splash` field distinguishes this from the main suite's generic method description.

## Main results

Values are medians of three run-level observations. Memory observations within each run are themselves medians of samples taken during the idle window. CPU percentages are relative to **one logical core**, not the entire 16-CPU machine.

| Sessions | Jcode PSS MiB | Maki PSS MiB | Jcode private MiB | Maki private MiB | Jcode processes | Maki processes |
|---:|---:|---:|---:|---:|---:|---:|
| 1 | 218.8 | 67.9 | 137.4 | 67.9 | 2 | 1 |
| 5 | 271.5 | 211.0 | 156.3 | 178.5 | 6 | 5 |
| 10 | 327.0 | 394.9 | 211.3 | 362.5 | 11 | 10 |

PSS ranges across repetitions: Jcode 216.6 to 219.4 / 267.0 to 273.1 / 324.5 to 327.6 MiB. Maki 67.6 to 69.5 / 204.9 to 215.3 / 387.9 to 395.0 MiB, for 1 / 5 / 10 sessions respectively.

Summed RSS medians were Jcode 308.7 / 804.5 / 1414.9 MiB and Maki 67.9 / 340.8 / 686.9 MiB. **Summed RSS double-counts shared resident pages**, so it is not the preferred total-physical-memory metric for multiple processes. PSS apportions shared pages, while private memory excludes shared pages entirely. PSS can also vary with unrelated processes mapping the same executable on this active laptop.

| Sessions | Jcode idle CPU % | Maki animated welcome CPU % | Jcode input echo ms | Maki input echo ms |
|---:|---:|---:|---:|---:|
| 1 | 0.53 | 3.63 | 3.41 | 4.71 |
| 5 | 2.18 | 15.61 | 3.61 | 3.60 |
| 10 | 7.55 | 34.51 | 4.03 | 3.64 |

Jcode emitted zero terminal bytes throughout every main idle measurement. Maki's animated welcome emitted a median 52 / 116 / 239 KiB/s across 1 / 5 / 10 terminals. This is terminal output volume, **not network traffic**. The harness parses that output in a separate process, whose CPU is not charged to either tool. That parser activity can still affect scheduling on the host.

### Startup

Fresh-process startup uses warm filesystem caches. This is not a reboot-cold or dropped-page-cache test. Jcode's first-session total includes server startup, with socket-connectability polled at 2 ms intervals. A connectable socket alone is not sufficient: the client must then render a prompt and echo the probe.

| Metric | Jcode | Maki |
|---|---:|---:|
| First editable session, one-session runs, median | 71.2 ms | 138.4 ms |
| Same, observed range | 69.6 to 103.8 ms | 106.4 to 233.0 ms |
| First editable session, all nine starts, median | 71.2 ms | 127.4 ms |
| Additional client, five-session runs, pooled median | 25.1 ms | 137.6 ms |
| Additional client, ten-session runs, pooled median | 24.9 ms | 110.0 ms |

Startup and input-echo measurements include Python subprocess/terminal-parsing overhead and approximately 2 ms polling granularity. Differences of a fraction of a millisecond should not be treated as meaningful.

## Splash-disabled Maki control

| Sessions | Maki PSS MiB | Maki private MiB | Maki idle CPU % | Observed CPU range | Maki input echo ms |
|---:|---:|---:|---:|---|---:|
| 1 | 67.7 | 67.7 | 0.52 | 0.51 to 0.68% | 3.74 |
| 5 | 212.0 | 179.1 | 2.60 | 2.42 to 2.69% | 3.36 |
| 10 | 390.9 | 358.0 | 5.27 | 4.72 to 5.42% | 3.26 |

Maki emitted no idle output in eight control runs and 140 bytes total in one five-session run. The animation-off result strongly suggests the welcome animation explains most of the main suite's extra Maki CPU and terminal output. Memory conclusions are essentially unchanged.

Jcode's CPU ranges were 0.18 to 0.72% / 2.08 to 2.38% / **4.39 to 17.85%** at 1 / 5 / 10 sessions. That broad final range and the short observation windows prevent a strong steady-state CPU ranking. The control was also a later suite, not interleaved with Jcode.

## Reproduction and artifacts

Harness: `scripts/bench_jcode_maki.py`. Tests: `scripts/test_bench_jcode_maki.py`.

The full raw JSON, including startup samples, typing samples, screen evidence, paths, hashes, and executable checks, is retained locally at:

```text
/home/jeremy/.jcode/scratch/maki-bench/results.json
/home/jeremy/.jcode/scratch/maki-bench/no-splash.json
```

Reproduce on Linux with Python 3.11+ and `pyte` installed in an isolated environment:

```bash
python scripts/bench_jcode_maki.py \
  --jcode /path/to/pinned/jcode --maki /path/to/pinned/maki \
  --runs 3 --sessions 1 5 10 --idle-seconds 5 --output results.json

python scripts/bench_jcode_maki.py \
  --jcode /path/to/pinned/jcode --maki /path/to/pinned/maki \
  --tools maki --maki-no-splash \
  --runs 3 --sessions 1 5 10 --idle-seconds 5 --output no-splash.json

python -m unittest discover -s scripts -p test_bench_jcode_maki.py -v
```

Validation: all **27 runs completed**, every recorded executable matched its pinned path, and all **five harness tests passed**. Tests cover live/missing process accounting, application input echo, rejection of unrendered input as readiness, and owned-process cleanup.

## Limitations and next useful measurements

- No model requests, real coding tasks, repository indexing workloads, tool execution throughput, or long-chat memory growth were measured.
- No headless/swarm-worker comparison was performed. Multiple idle TUI sessions are not equivalent to active swarm workers.
- Fresh minimal configurations are not necessarily either tool's typical configured installation. Jcode memory was explicitly disabled.
- The active laptop had other development activity, no CPU pinning, and no frequency or thermal control. Three repetitions are descriptive rather than statistically conclusive.
- Peak startup memory, energy consumption, network transfer, context/token efficiency, and task success rate are not covered.
- A stronger task benchmark would use matched model/provider settings, identical repository snapshots and prompts, and repeated successful tasks, reporting model latency and token usage separately from local overhead.
