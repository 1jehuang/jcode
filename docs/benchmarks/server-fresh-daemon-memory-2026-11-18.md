# Fresh daemon memory investigation, 2026-11-18

## Scope and acceptance

Measure a fresh daemon with no connected sessions separately from transcript-heavy workloads. Preserve history, provider discovery, concurrency, image fidelity, first-request readiness and transport behavior. A smaller heap obtained by postponing useful work is not accepted as a pure efficiency. Neither a 20 MiB nor a 50 MiB target is assumed achievable.

Measurements use a dedicated empty `JCODE_HOME`, `JCODE_TEST_SESSION=1`, unique runtime/socket paths, fake OpenAI credentials and a loopback-only provider endpoint. No prompts or inference requests are sent. The normal executable is pinned at `/home/jeremy/.jcode/scratch/ram-fork/runtime-20261118-owned-after` (d103e8a62 production sources). Scratch harness: `fresh-daemon-probe.py` in that directory. Samples are taken after readiness, three seconds later, and another twelve seconds later. Readiness uses a 200 ms polling interval, so it is not a precise startup latency benchmark.

## Initial attribution, before new startup changes

`fresh-normal1-20261118` settled sample:

| Metric | Bytes | MiB |
| --- | ---: | ---: |
| RSS | 95,834,112 | 91.39 |
| PSS | 91,826,176 | 87.57 |
| Anonymous PSS | 16,719,872 | 15.95 |
| Live allocator bytes | 3,420,176 | 3.26 |

This is a selfdev executable, not a distribution optimization profile. It is not evidence that the daemon has a 90 MiB live heap. `smaps-12.txt` attributes 56,784 KiB to resident executable instructions, 16,524 KiB to the executable's read-only mapping, 5,920 KiB to relocated read-only pages, and 2,928 KiB to the main-thread stack. File-backed clean pages are reclaimable and can be shared by processes using the same executable inode. A private copied benchmark executable should not be used to infer the PSS sharing of the user's live daemon.

`fresh-normal2-20261118` is an independent repeat. These initial controls use temporary-server policy and disable memory consumers. A separate `fresh-regular-before1-20261118` control retains default feature/background policy in the same isolated empty home. The tool profile is `none` in both, and no sessions are created.

An installed older stable executable was measured only as a reference: `fresh-stable-reference-20261118`, approximately 48 MiB RSS at readiness. Its source revision and build configuration differ. **This is not a measured optimization of the current code**, nor a reason to replace the current runtime with an old binary.

## Sampled startup allocation lead

A separate `jemalloc-prof` executable, digest `3d312fee01b1ada46dab7863a26f16a042600068b4ecf7132a6652f881b605b1`, produced an exit profile without creating a synthetic session: `fresh-prof3-20261118/startup-final.2591502.0.f.heap`. `prof_final:true` captures after orderly shutdown, so its retained 2.3 MiB is **not** a live idle-heap measurement.

Cumulative allocation was 28.9 MiB. Native certificate loading accounted for 13.2 MiB across startup client creation. Two daemon loops independently construct identical default `reqwest::Client` instances: mobile transcript sync (4.5 MiB cumulative attribution) and local mobile command delivery (4.9 MiB). Sharing those identical clients is under implementation and validation. Provider HTTP clients have different settings and are not substituted. No TLS trust, authentication, timeout or cookie settings may be weakened.

## Open gates

- Measure the same source under an optimized build before attributing executable-page overhead to application logic.
- Measure CLI future layout before attempting stack changes. Boxing merely moves memory and is not automatically an improvement.
- Validate shared mobile transport exact behavior, optimized CPU and allocation costs, then repeat startup and first-request acceptance.
- Revisit concrete remaining owners after changes. Do not claim global exhaustion or lower total RSS from microbenchmarks alone.
