# Search traversal probe, 2026-11-18

## Accepted change

Only `session_search_raw_matches_query`'s ASCII term fallback changes. It now stops at the requested matching-term threshold, with an explicit zero-threshold guard. The actionable check, exact-match fast path, invalid-UTF8 handling, non-ASCII conversion, empty/duplicate term behavior and public custom thresholds remain unchanged. The scoring and normalized-text matching functions are unchanged.

The exact legacy oracle is frozen from commit `0fd761c3c`. Correctness checks compare raw and normalized booleans plus score floating-point bits, snippets, exact flags, and matched-term order across custom profiles (including zero and impossible thresholds, no terms, empty terms, duplicates, Unicode, and invalid UTF8).

## Method

Run:

```
justrust check -p jcode-session-types --tests
justrust test -p jcode-session-types
justrust test -p jcode-session-types --release --test search_traversals search_traversal_probe -- --ignored --nocapture
```

All checks ran hosted. Full crate suite: 15 passed, 2 ignored. The optimized probe is deliberately ignored in ordinary tests. It alternates legacy/new execution order for 21 rounds and reports median nanoseconds per call measured by actual Linux `CLOCK_THREAD_CPUTIME_ID`, not wall time. The separate integration-test executable has one counting allocator, without introducing a second allocator into the existing library-test binary. Every workload asserts no allocation-call or allocated-byte increase.

Final hosted optimized run: `20261118-014152154-1709851`.

| Raw search workload | Legacy ns | Accepted ns | Ratio |
| --- | ---: | ---: | ---: |
| Single-byte exact hit | 8 | 7 | 0.875 |
| Single-byte no hit | 10 | 9 | 0.900 |
| Overlapping terms | 23 | 23 | 1.000 |
| Exact phrase | 12 | 12 | 1.000 |
| Term-only | 32 | 32 | 1.000 |
| No hits | 14 | 14 | 1.000 |
| Many terms | 83 | 31 | 0.373 |
| Large ASCII, early hits | 155498 | 77805 | 0.500 |
| Large ASCII, late hits | 233888 | 156158 | 0.668 |
| Large ASCII, no hits | 233204 | 233170 | 1.000 |
| Large Unicode, early hits | 838836 | 838859 | 1.000 |
| Large Unicode, late hits | 846739 | 846811 | 1.000 |
| Repeated hits | 213863 | 213977 | 1.001 |

All ASCII raw cases allocate zero times and zero bytes in both versions. Unicode early allocates 2 calls / 540036 bytes in both. Unicode late allocates 2 calls / 540033 bytes in both. The 0.1% repeated-hit difference is within repeated-run variation (previous ratios 1.000, 1.000, 1.000). No reproducible raw-search regression was observed. Earlier independent optimized runs measured many-terms 82/30 and 84/30 ns, early ASCII 155475/77805 and 153768/76785 ns, late ASCII 233030/155771 and 231596/154937 ns.

## Rejected candidates

- `match_indices` first position plus remaining count preserved all oracle outputs, including empty-term nonoverlap semantics, but repeated-hit scoring moved from 116706 to 128135 ns (1.098) in an expanded run. Do not ship it on these results.
- `find` followed by counting only the suffix after the first nonempty match also preserved oracle outputs, and helped late ASCII scoring (74777 to 46057 ns), but regressed short exact scoring (130 to 142 ns) and term-only scoring (128 to 134 ns). Empty terms needed their original count path. Rejected.
- `nth(min_term_matches - 1)` for normalized strings helped many-term and large-buffer cases, but short two-term and no-hit runs regressed substantially. A variant retaining full counting when all terms were required still measured 43 to 50 ns for no hits. Rejected, and normalized matching restored verbatim.

CPU timings for unchanged scoring/normalized functions also varied with code placement and hosted load. Consequently those microbenchmark differences are not claimed as improvements, and no guarantee is made about arbitrary unmeasured hardware. Only the repeatable raw-search savings above are claimed.
