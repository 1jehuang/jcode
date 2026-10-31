# Exact resume resolution, 2026-10-27

## Scope

`find_session_by_name_or_id` now resolves existing exact snapshot stems using file metadata instead of parsing a startup stub and, on failure, fully loading the transcript. The authoritative session load still detects corruption, performs backup recovery, and replays the latest journal. Name/title searches retain their existing metadata and journal behavior. Exact filename lookup rejects path separators, NUL, and dot components, and requires a regular file. Imported exact IDs and bare-provider aliases remain supported.

No startup metadata sidecar was added. A sidecar needs coordinated snapshot/journal invalidation and write-path changes, while this scoped change removes one redundant whole-snapshot read without introducing a second metadata authority. Other startup-stub reads remain and can be optimized separately.

## Read-only real local corpus measurement

Measured on the user's Linux x86_64 Intel Core Ultra X9 388H machine with `justrust test --local`, debug/test profile, nine serial samples per session. The baseline is the previous successful exact-ID path, `Session::load_startup_stub(id)`. The new measurement calls the real `find_session_by_name_or_id(id)`. Both validate successful resolution. No session save or full loader is invoked in this benchmark, no disk cache is evicted, and each snapshot's size and mtime are checked unchanged afterward. Cache state is uncontrolled, with repeated samples normally warm.

These are **resolution-helper timings, not end-to-end resume or first-frame latency**. The server restore, other startup metadata reads, process launch, and TUI rendering are outside this measurement. The baseline and optimized helper are alternated in that order. Timings are microseconds, and `pmax` is the maximum of nine observations, not a statistically robust tail estimate.

| Real session | Snapshot bytes | Old stub p50 | Old max | New exact lookup p50 | New max | First old sample |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `session_iwazaru_1792093867747_93935c5f67d3dbf1` | 44,722,554 | 28,763 | 31,852 | 10 | 17 | 31,852 |
| `session_chipmunk_1792210829874_6ac925de8309f485` | 44,698,478 | 29,018 | 36,465 | 13 | 19 | 36,465 |
| `session_gorilla_1792231005473_85e0a9c7aa40cfda` | 41,656,962 | 31,223 | 32,349 | 14 | 27 | 30,170 |

Reproduce with existing local session IDs only:

```bash
JCODE_RESUME_BENCH_IDS=<id1>,<id2>,<id3> justrust test --local -p jcode-base --lib benchmark_exact_resume_resolution_real_corpus -- --ignored --nocapture
```

Local corpus run: `20261027-010959569-2287748`. Hosted compilation check passed (`20261027-010909096-2284101`). Final targeted hosted regression run `20261027-011103849-2294297` passed ten tests, including journal-only metadata changes, authoritative corruption detection, traversal/directory rejection, imported IDs, bare-provider aliases, and title matching. The broader hosted session suite `20261027-011145906-2298307` passed 112 tests with three opt-in tests ignored.
