# SQLite session search follow-up

This is a separate change on top of the opt-in session-storage backend. It does not switch the default backend or migrate a live session directory.

## Scope

- Native SQLite search candidates come from a transactionally maintained FTS5 index instead of rebuilding the file backend's separate Bloom index.
- The file backend and exhaustive search remain available. Existing result scoring, filters, snippets, and candidate limits remain in the application.
- No-query picker listing applies the visible/debug budgets and saved-session rules in SQL before fetching summary JSON. It reads metadata for rows that can appear, not transcript bodies or every summary in the discovery window.
- SQLite commits already store changed rows without producing full JSON snapshots. JSON import/export remains the compatibility interface.

FTS5 is an index, not an index-free search method. Building it takes time and storage, and maintaining it adds write work. Query latency must be measured separately from schema migration and initial indexing. The index retains trigram positions so quoted phrases can verify contiguous substrings without reading external transcript content. Existing bounded result scoring still loads matching sessions. Short and Unicode queries retain the existing Bloom/raw fallback rather than silently changing search semantics.

## Preparing the optional index

Preparation is explicit, not part of opening the store, listing sessions, saving a turn, or searching. Stop writers against the target store first. The initial build holds a write transaction and can take many minutes on a multi-gigabyte corpus. Until preparation completes, searches use the existing fallback. A completed index is maintained transactionally by subsequent writes.

```sh
JCODE_HOME=/path/to/offline/store cargo run -p jcode-base --example session_search_index
```

The same operation is available as `SessionStore::prepare_search_index()`. Calling it again on a prepared store does not rebuild the index. Do not use a live store for benchmarking.

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

## Reproducing search measurements

Build `session_search_bench` from both revisions using the same profile and compiler. Prepare separate copies of the same SQLite corpus. Keep query strings in a private JSON object mapping public category labels to strings.

```sh
cargo build -p jcode-app-core --example session_search_bench --no-default-features --features test-support
python3 scripts/bench_session_search.py BEFORE_BINARY AFTER_BINARY BEFORE_HOME AFTER_HOME \
  --queries PRIVATE_QUERY_FILE --repeats 5 > search-results.json
```

The driver alternates fresh processes, reports aggregate wall/tool timings and result counts, and fails if result digests differ. It does not publish queries or transcript contents. It does not evict the host page cache. An initial indexing run belongs in its own measurement rather than being mixed into warm-query samples.
