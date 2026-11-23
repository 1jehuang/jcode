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

Cumulative allocation was 28.9 MiB. Native certificate loading accounted for 13.2 MiB across startup client creation. Two daemon loops independently construct identical default `reqwest::Client` instances: mobile transcript sync (4.5 MiB cumulative attribution) and local mobile command delivery (4.9 MiB). Sharing those identical clients was implemented and accepted as `bbd9a2d66` after exact request-policy tests and optimized CPU/allocation checks. Provider HTTP clients have different settings and are not substituted. No TLS trust, authentication, timeout or cookie settings may be weakened.

## Broader controls and caveats

The first harness inherited the shared temporary background-task registry. Subsequent acceptance controls set a private `TMPDIR=<work>/tmp` as well. No global task files were modified. Use `fresh-isolated-before1-20261118` and `fresh-isolated-before2-20261118` for that stricter old-runtime control, and `fresh-isolated-opt1-20261118` for the optimized current executable. Exact binary digests and all raw memory samples remain alongside each fixture.

A completed hosted optimized executable was pinned only after artifact delivery: SHA-256 `553b2fca0e4ca7ac1550c1ba69a6e30ef7352ac245008b3d9a1ea59713361ded`. A premature earlier copy, `fresh-release-opt1-20261118`, had a different digest and is excluded from current-build evidence. These controls include the newly shared mobile client and other integrated code, so they are not a strict build-profile-only comparison against d103e8a62.

A distribution-profile control (`release-lto`, hosted build completed in about 199 seconds) was also warranted because executable pages dominate the target. Pinned digest `c0961cf3ff65bc79a79a6dc45d05ad0feac796066982d3416330e31e17d3f8a4`, private regular-daemon runs `fresh-distribution1-20261118` and `fresh-distribution2-20261118`: final RSS **69.60 / 68.93 MiB**, anonymous PSS **7.95 / 7.39 MiB**, live allocator bytes **2,392,224 / 2,398,272**. This supports an approximately 70 MiB observation for that build, not a 20–50 MiB achievement. It does not change the user's selected runtime or default build profile.

The layout probe measured an 82,896-byte entry future, 82,912 bytes with the original await-only wrapper, and a 312-byte `BusEvent`. Removing the wrapper was considered but withdrawn without claiming a measured startup benefit. Boxing by itself is not a demonstrated memory reduction.

The normal daemon reports the **system allocator**. `MALLOC_CONF=prof:false` is a fixture setting, not evidence that the ordinary executable uses jemalloc. Only the explicitly profiled build uses sampled jemalloc data. Allocation counters, RSS, PSS, executable mappings and post-shutdown sampled retention describe different quantities.

## Matched executable-cache diagnostic

The repeated parser-variant RSS difference was challenged using only private pinned scratch executables. `startup-cache-control.py` reads each entire file before warm controls, or uses `fsync` plus `POSIX_FADV_DONTNEED` on that private file before advisory cold controls. No global cache drop, running production file, or production setting was touched.

Final RSS for control/variant: warm **54.69 / 47.37 MiB**, advisory cold **46.98 / 47.49 MiB**. Live allocator bytes remained **2,400,448 / 2,405,888 / 2,406,928 / 2,390,864**. Raw fixtures: `fresh-{warm,cold}-{control,clap3}-20261118`. Under matched advisory-cold conditions the previously apparent ~20 MiB improvement disappears. This is strong evidence that executable page-cache/fault residency contaminated the original comparison, not evidence of a 20 MiB heap optimization. The advice is not proof of a perfectly cold disk cache. Neither production cache eviction nor delayed useful work is proposed as an optimization.

## Open gates

- Shared mobile transport is accepted as `bbd9a2d66`, with exact local wire/pooling/auth/cookie coverage, 18 relevant tests and optimized actual thread-CPU/separate allocator measurements. Its 47.4% CPU and 4.64 MiB cumulative-allocation savings describe **the duplicate client-construction primitive**, not the entire daemon or its retained heap. See `startup-mobile-http-2026-11-18.md`.
- Same-source parser-dependency optimization was investigated with experimental `clap_builder`/`clap_lex` opt-level 3 overrides, without changing default profiles. Exact CLI exit/stdout/stderr parity passed five fixtures in two optimized release runs (16 alternating pairs per fixture). Release `serve --help` CPU ratios were 1.0667 and 1.0423, so the experiment is **not accepted as a pure efficiency**. Selfdev CPU results were better, but do not override release nonregression evidence.
- A release startup sample showed 46.91 MiB RSS for the variant versus 66.83 MiB for its control, while live heaps were effectively equal (~2.4 MB). Total executable sections differed by only 29,035 bytes. The apparent ~20 MiB resident difference is therefore not a demonstrated removal of 20 MiB of required state. File-page residency/layout requires further attribution before any RAM claim. Raw controls: `startup-release-cli-final-20261118`, `startup-release-cli-repeat-20261118`, `fresh-release-clap3-final1-20261118`, `fresh-release-control-final1-20261118`. Reverse-order repeats (`fresh-release-control-reverse2-20261118`, `fresh-release-clap3-reverse2-20261118`) measured 65.82 / 47.98 MiB RSS, with live heaps 2,416,480 / 2,414,432 bytes. Thus the residency difference repeats in these controls, but is not a heap saving: first-pair `smaps` attributes 18,684 KiB of the difference to clean executable instructions and 1,592 KiB to clean read-only pages, with effectively equal anonymous state. This does not pass the CPU gate or establish its cause. No default optimization settings changed.
- Revisit concrete remaining owners after changes. Do not claim global exhaustion or lower total RSS from microbenchmarks alone.
