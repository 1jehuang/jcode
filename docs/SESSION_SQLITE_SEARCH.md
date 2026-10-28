# SQLite session search follow-up

This is a separate change on top of the opt-in session-storage backend. It does not switch the default backend or migrate a live session directory.

## Scope

- Native SQLite search candidates come from a transactionally maintained FTS5 index instead of rebuilding the file backend's separate Bloom index.
- The file backend and exhaustive search remain available. Existing result scoring, filters, snippets, and candidate limits remain in the application.
- No-query picker listing applies the visible/debug budgets and saved-session rules in SQL before fetching summary JSON. It reads metadata for rows that can appear, not transcript bodies or every summary in the discovery window.
- SQLite commits already store changed rows without producing full JSON snapshots. JSON import/export remains the compatibility interface.

FTS5 is an index, not an index-free search method. Building it takes time and storage, and maintaining it adds write work. Query latency must be measured separately from schema migration and initial indexing. Positional mode retains trigram positions so quoted phrases can verify contiguous substrings without reading external transcript content. Compact and column modes use indexed `LIKE` and verify candidates against external content. Existing bounded result scoring still loads matching sessions. Short and Unicode queries retain the existing Bloom/raw fallback rather than silently changing search semantics.

## Preparing the optional index

Preparation is explicit, not part of opening the store, listing sessions, saving a turn, or searching. Stop writers against the target store first. The initial build holds a write transaction and can take many minutes on a multi-gigabyte corpus. Until preparation completes, searches use the existing fallback. A completed index is maintained transactionally by subsequent writes.

```sh
JCODE_HOME=/path/to/offline/store cargo run -p jcode-base --example session_search_index -- compact
# Alternative modes: column or positional (the default).
```

The same operation is available as `SessionStore::prepare_search_index_mode(mode)`. `prepare_search_index()` selects positional mode for compatibility. Repeating the same mode does not rebuild the index. Changing modes explicitly rebuilds it in a transaction. Ordinary opens recognize all three modes without rebuilding. New indexes omit per-row token counts (`columnsize=0`). Do not use a live store for benchmarking.

## Validation contract

Use the same frozen corpus for the old and new SQLite implementations. Keep both databases private. Compare actual `SessionSearchTool::execute` output, excluding only the diagnostic scanned-candidate count, and reject unsuccessful tool responses. Compare the actual picker `load_sessions()` row count and displayed-metadata digest.

Report separately:

1. Schema migration and initial index construction time and resulting database/WAL size.
2. Fresh-process, warm-filesystem no-query picker latency with equal rows.
3. Absent, selective, substring, multi-term, broad, and short queries through the public search tool.
4. Mutation visibility after append, replacement, truncation, metadata changes, and reopen.
5. File-backend behavior and indexed-versus-exhaustive search parity.

A broad query still needs to load matching sessions to generate results. Short or unsupported query forms may require conservative candidates. No claim of constant-time search, index-free search, or universal latency improvement is intended.

## Corpus measurements (2026-10-28)

The frozen private corpus contains 3,018 sessions. Both revisions use the same compiler and unoptimized development profile, alternate fresh processes, and use a warm filesystem cache. External sessions are disabled. These are local measurements, not portable performance guarantees.

| Public operation | Before median | After median | Result comparison |
| --- | ---: | ---: | --- |
| No-query picker, seven runs | 727.8 ms | 287.7 ms | Same 800 rows and metadata digest |
| Absent search, five runs | 1,272.9 ms | 70.8 ms | Same zero results |
| Selective search, five runs | 1,661.2 ms | 1,551.0 ms | Same eight results |
| Broad search, five runs | 996.4 ms | 1,449.5 ms | Same 50 results, slower |
| Short search fallback, five runs | 1,705.1 ms | 1,746.5 ms | Same 50 results |
| Substring search, five runs | 1,357.6 ms | 2,347.4 ms | 16 versus 50 results, not equivalent work |
| Multi-term search, five runs | 2,842.7 ms | 2,305.2 ms | Both capped at 50, different result digests |

The comparison driver deliberately exits unsuccessfully for the last two queries. Their timings do **not** establish an equivalent-work speedup. A read-only audit compared the actual baseline Bloom cache and SQLite FTS candidates with raw persisted-text matches. For the substring query, 491 sessions matched raw text: Bloom omitted 475 and admitted 29 nonmatches, while FTS matched all 491 with no extra candidates. For the multi-term query, 224 sessions matched raw text: Bloom omitted 35 and admitted 20 nonmatches, while FTS matched all 224 exactly. The baseline indexes whole tokens rather than arbitrary substrings, explaining these recall differences. These are session-candidate counts, not final message-result counts. Existing bounded scoring still affects final ranking.

Exhaustive public search on both revisions returned identical 50-result digests for both queries in three alternating runs. Its medians were 8,788/8,899 ms for substring and 7,244/7,463 ms for multi-term (before/after). FTS candidate recall is correct for the two audited queries, but bounded indexed result sets intentionally differ from the old token prefilter. This remains an experimental follow-up because of its setup/storage costs and broad-query regression.

Initial positional index preparation took **1,237.8 seconds (20 minutes 38 seconds)**. The checkpointed database grew from **3,476,684,800 to 11,010,162,688 bytes**, about 3.17 times the original size. No compression was added. An already-prepared index check took 0.13 ms, excluding store open and schema migration. This setup cost is why preparation is explicit and offline.

On a separate copy, 21 metadata-only commits had a median of 13.3 ms before and 22.7 ms after indexing. The 21 append commits had medians of 16.6 ms and 15.0 ms. After-index maxima were 600.1 ms and 213.3 ms respectively. These small, non-paired mutation samples do not establish a write-throughput improvement. Reopened session values matched after every commit.

Focused validation: 11 store tests, 28 search tests, and four SQLite picker tests passed. One pre-existing search benchmark test remained ignored. This is not a full-workspace test claim.

## Index alternatives on the same sample

`session_index_variants` compares real persisted rows from 64 size-quantile sessions: 12,651 rows, 75,824,139 payload bytes, and an 81,063,936-byte base database. Five sessions larger than 32 MiB were excluded from sampling. All variants use the same rows, SQLite 3.46.0, external content, WAL with FULL synchronization, and three query repeats. Modes run in fixed order, so this is screening evidence, not a randomized full-workload speed claim.

| Variant | Additional index bytes | Build seconds | Absent ms | Substring ms | Same candidates for all six queries |
| --- | ---: | ---: | ---: | ---: | --- |
| Scan | 0 | 0 | 105.2 | 162.5 | Yes |
| B-tree on text | 90,255,360 | 1.0 | 103.9 | 157.6 | Yes |
| Word FTS | 40,095,744 | 2.4 | 0.08 | 0.06 | **No** |
| Compact trigram | 17,956,864 | 12.7 | 16.3 | 20.9 | Yes |
| Column trigram | 44,380,160 | 24.4 | 13.4 | 24.0 | Yes |
| Positional trigram | 167,825,408 | 19.0 | 0.81 | 0.44 | Yes |

Word FTS omitted all eight substring-matching sessions and nine of the 62 short-query matches. Its lower times do not represent equivalent work. The B-tree did not help the leading-wildcard workload. Compact trigram used 55% less index space than word FTS and 89% less than positional trigram on this sample, while retaining the tested substring semantics. Positional remained faster for indexed candidate selection. Column mode was not a clear win over compact in these measurements.

The example also records three small-row update commits per variant and validates read-back. Those samples are too small and order-dependent for a write-throughput conclusion. Aggregate measurements are in `docs/benchmarks/session-index-variants-2026-10-28.json`.

```sh
cargo run -p jcode-base --example session_index_variants -- SOURCE_DB NEW_OUTPUT_DIR PRIVATE_QUERIES_JSON
```

The source database is opened read-only. The output directory must not exist and contains private copied content. Only aggregate counts, sizes, timings, and comparison results are printed. No live configuration or session data is changed.

## Compact versus positional on the full corpus

The existing prepared copies of all 3,018 sessions occupy **4,710,825,984 bytes (compact)** and **11,010,162,688 bytes (positional)** after checkpointing. Compact saves 57% of total database space in these copies. These earlier-built indexes retain token-count tables, unlike the new sample's `columnsize=0` builds, so their sizes must not be extrapolated from the sample.

Three alternating fresh-process runs through `SessionSearchTool::execute` produced identical result digests in all six categories:

| Query category | Positional median ms | Compact median ms | Results |
| --- | ---: | ---: | ---: |
| Absent | 3,920.9 | 1,823.6 | 0 |
| Selective | 3,455.6 | 6,551.5 | 8 |
| Substring | 3,907.2 | 6,353.2 | 50 |
| Multi-term | 4,016.7 | 8,836.2 | 50 |
| Broad | 2,231.8 | 9,556.1 | 50 |
| Short fallback | 2,679.1 | 3,135.7 | 50 |

Timing variation was large: positional absent-query samples ranged from 164.7 to 15,540.5 ms. Do not infer that compact is faster for absent queries, or compare this run directly with earlier measurements under different contention. The useful result is output parity and a measured space/latency tradeoff: compact saves disk but generally does more work verifying candidates. The positional default is unchanged; compact and column remain explicit offline choices.

The current changes passed 11 store tests and 28 search tests (one existing benchmark ignored), including compact/column mode switching, append visibility, truncation, and rejection of noncontiguous trigram false positives. The full-corpus public comparison is saved in `docs/benchmarks/session-compact-vs-positional-2026-10-28.json`. No live sessions, configuration, or installed binaries were changed.

## Reproducing search measurements

Build `session_search_bench` from both revisions using the same profile and compiler. Prepare separate copies of the same SQLite corpus. Keep query strings in a private JSON object mapping public category labels to strings.

```sh
cargo build -p jcode-app-core --example session_search_bench --no-default-features --features test-support
python3 scripts/bench_session_search.py BEFORE_BINARY AFTER_BINARY BEFORE_HOME AFTER_HOME \
  --queries PRIVATE_QUERY_FILE --repeats 5 > search-results.json
```

The driver alternates fresh processes, reports aggregate wall/tool timings and result counts, and fails if result digests differ. It does not publish queries or transcript contents. It does not evict the host page cache. An initial indexing run belongs in its own measurement rather than being mixed into warm-query samples.
