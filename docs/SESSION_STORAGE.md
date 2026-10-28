# Session storage backends

## Scope

This change separates session persistence from the agent's in-memory `Session` model. The existing JSON snapshot plus JSONL journal remains the default. The SQLite backend is opt-in. Neither the benchmark nor the migration removes source sessions.

The storage boundary owns session discovery, metadata summaries, loading, and committing a session's changed state. It does not own provider calls, context compaction, rendering, process presence, credentials, or global configuration. A future backend must preserve the same session semantics, including replacement and truncation, not only appends.

## Compatibility

File import/export remains an interchange format, not a requirement to mirror each SQLite write to a JSON snapshot. Older binaries cannot read SQLite-only sessions. Retain an export before reverting a session to a file-only binary. Metadata listing must not deserialize message bodies. Session commits must update metadata and changed transcript rows atomically.

Any caller that still enumerates the `sessions` directory directly must be identified before treating SQLite as a replacement for every workflow. Default behavior must remain unchanged for users who do not opt in.

The file backend's checkpoint-specific environment-history trimming is unchanged. SQLite retains imported environment rows until the caller replaces that history. To compare steady-state writes with identical inputs, the benchmark checkpoints the 16 file samples once and mirrors their retained environment history into the SQLite samples before timing. The full corpus import equality check and untouched picker copies precede this preparation.

The original backend benchmark below did not include compression or an FTS index. The optional [SQLite search follow-up](SESSION_SQLITE_SEARCH.md) adds explicit offline FTS preparation and bounded picker metadata queries. The original timings remain the pre-FTS baseline. File-only SDK and auxiliary analytics consumers remain compatibility work until exercised against database-only sessions.

## Usage and integration boundary

`SessionStore::open(root, SessionBackend::Files | SessionBackend::Sqlite)` selects an explicit store. `SessionStore::configured()` uses `JCODE_HOME` and `JCODE_SESSION_BACKEND` (default `files`). `SessionStorage` is the backend trait. The agent still owns an in-memory `Session`; a backend commits its changed state atomically rather than receiving arbitrary SQL from callers.

For an isolated experiment, start both the daemon and client with the same private `JCODE_HOME` and `JCODE_SESSION_BACKEND=sqlite`, and use a separate socket. Setting the variable on a client does not reconfigure an already running daemon. Do not point an experimental process at a live session directory.

The SQLite store lazily imports a legacy session when loaded. For bulk migration, call `import_from_path` on each existing snapshot. It replays the adjacent JSONL journal without repairing or deleting source files, preserves timestamps and unknown metadata, and does not overwrite an existing database session. Normal SQLite commits do not create JSON shadow snapshots. `export(id, path)` writes a self-contained, owner-readable JSON snapshot, and `/transcript [path]` exports a SQLite session before opening it.

The picker, session search, exact/name resume lookup, crash recovery, and replay use the store for SQLite sessions. Existing file-backed sessions remain discoverable alongside imported records; the picker skips parsing source snapshots already represented in the database. SDK/harness APIs, auxiliary analytics, cloud transfer, and other direct-file consumers are not all converted. This is an experimental backend, not a claim that every Jcode workflow supports database-only sessions.

Import preserves the latest snapshot/journal modification time as separate discovery metadata. The picker uses that ordering, including its debug-session budget and saved-session tail, rather than substituting the session's content timestamp and silently selecting different rows. Import also preserves a missing stored title instead of persisting the title that a normal resume derives from the first prompt. Both pickers therefore retain the same display fallback; normal session loading still derives a missing title. Ordinary SQLite commits refresh the discovery timestamp. This metadata is not added to exported session JSON.

## Benchmark contract

The comparison uses the same captured session corpus and public storage operations for both implementations. Only aggregate results may be published. Do not commit transcript content, session identifiers, prompts, working directories, credentials, or the private corpus.

Report separately:

- Source import time separately from steady-state reads. Label process peak memory as a whole-run measurement, not import-only memory.
- First-process and repeated metadata listing, at equal result limits.
- Full resume loads for the same sessions, stratified by serialized size.
- Ordinary activity-metadata commits, title-edit commits (the file backend chooses journal or snapshot by size), one-message append commits, and history replacement/truncation.
- Reopen and read-back validation after commits.
- Allocated and logical storage including SQLite database, WAL, and shared-memory files, and file snapshots plus journals.
- Durability settings and transaction boundaries. Do not compare a buffered append with a durable commit without identifying the difference.

SQLite uses WAL mode, `synchronous=FULL`, and a five-second busy timeout. The current file backend uses `append_json_line_fast` (write/close without fsync) and `write_json_fast` (atomic replacement without durable fsync). The comparison preserves those existing choices. A SQLite write and a file append therefore do not offer equal crash durability, and latency figures must not be presented as an equal-durability contest.

Use repeated, alternating backend order and report medians and upper-percentile latency. A new process is not a cold filesystem cache. Do not clear the host's global page cache or interrupt other workloads. Report warm-cache results honestly and keep import/index construction out of query-only latency.

The acceptance check is equality of loaded sessions and mutations after reopening, not merely successful SQL execution. A speedup in listing does not establish a speedup in full resume, write throughput, search, memory use, or recovery.

## Reproducing the comparison

Use a private copy with `sessions/*.json` and their adjacent journals. The output directory must not exist. The storage example imports into `RUN/sqlite`, copies the same source to `RUN/files`, checks all loaded sessions, and then exercises both stores. An untouched `RUN/picker` copy keeps mutation timing from changing the later picker comparison. Legacy environment-retention cleanup of the 16 mutation samples is performed before steady-state timings. It never mutates `SOURCE`.

```sh
cargo build -p jcode-base --example session_storage_bench
cargo build -p jcode-tui --example session_picker_bench --no-default-features --features test-support
target/debug/examples/session_storage_bench SOURCE RUN 7 > storage-results.json
python3 scripts/bench_session_picker.py target/debug/examples/session_picker_bench RUN/picker --repeats 7 > picker-results.json
```

The examples and driver emit only aggregate results. Keep source copies, exports, and detailed diagnostics private. Record the compiler/profile, filesystem, corpus size, and concurrent host load with any published results. For an optimized comparison, build both examples with `--release` and use `target/release/examples/` consistently.

For parity diagnosis only, the picker example accepts an optional second argument naming an owner-readable private row dump. It contains session metadata and must not be published. Normal benchmark runs do not write this dump.

## Maintained validation

| Requirement | Public-path check |
| --- | --- |
| JSON interoperability | Import snapshot plus journal without modifying source, export to JSON, load with the file backend, compare persisted values |
| Incremental persistence | Append, replace, truncate, change metadata, reopen both backends, compare values |
| Concurrency | Reject conflicting transcript writers without partial commits; permit an owner to continue after a metadata-only side write |
| Privacy | Open and write under umask `0002`; verify the database and live WAL/SHM are owner-only |
| Discovery and recovery | Resolve a database-only session by ID/name, identify crashed state, recover it without a JSON snapshot |
| Picker | Compare real `load_sessions()` rows for both backends and for mixed legacy/database storage |
| Search | Find a database-only tool-body term in indexed and exhaustive modes, append a term and search again |
| Transcript export and replay | Run the TUI transcript command, open its JSON export, and load database-only sessions through replay |

The picker comparison calls the production `load_sessions()` fresh-scan path in a new process for each sample. It is not time to first cached frame, an interactive TUI render, a provider request, or total `jcode --resume` latency. Each backend uses the same scan limit and external-session setting. The driver refuses a speedup comparison unless row counts and a digest of displayed metadata match.

The examples measure the maintained public storage and picker APIs. They do not automate a live provider turn or prove every SDK/cloud integration supports SQLite. The private corpus and per-session outputs are not part of the repository.

## Measured workload: 2026-10-27

Source revision: `3299b7c89`, based on upstream master `a3ef562fc`. Both examples used the unoptimized development profile, rustc 1.98.1, an Intel Core i9-14900 (32 logical CPUs), Linux 6.12, and Btrfs. The host was shared with other work. These are warm-filesystem prototype measurements, not release-build or cold-start guarantees.

The frozen workload contained 3,018 snapshots and 402 journals, totaling 3,625,325,615 bytes. All 3,018 sessions imported without rejection and passed full persisted-value comparison. Every measured mutation was reopened and compared. Import took 167.15 seconds; the separate full-corpus comparison took 61.58 seconds. The whole run took 293.87 seconds with peak RSS 1,712,536 KiB, including both backends and validation. This is not SQLite-only memory usage.

### Real picker, equal results

Seven fresh processes per backend, alternating order, external sessions disabled, scan limit 400. Both backends returned the same 800 rows with identical displayed-metadata digests on every run. The second case retained all source JSON/journal files beside the SQLite database, as a non-destructive migration would.

| Case and metric | Files median / p95 | SQLite median / p95 |
| --- | ---: | ---: |
| Database-only: `load_sessions()` | 3,583 / 4,359 ms | 1,249 / 3,212 ms |
| Database-only: process wall time | 3,654 / 4,418 ms | 1,316 / 3,242 ms |
| Retained legacy files: `load_sessions()` | 3,419 / 4,556 ms | 1,146 / 8,131 ms |
| Retained legacy files: process wall time | 3,467 / 4,661 ms | 1,183 / 8,167 ms |

Median picker loading improved by 2.87x in the first case and 2.98x with retained source files. The latter had a slower SQLite tail sample; seven samples on a shared host are insufficient to claim a tail-latency improvement. The p95 here is the maximum of seven observations. These times do not include rendering or a provider request.

### Full loads and durable changes

Sixteen size-quantile sessions, including the largest, were sampled seven times in alternating backend order. Full loads were measured before checkpoint normalization. These quantiles deliberately cover large sessions, so the aggregate p95 is not a population-weighted session percentile. Values are milliseconds.

| Operation | Files median / p95 | SQLite median / p95 | Samples per backend |
| --- | ---: | ---: | ---: |
| Full session load | 2.10 / 1,097.03 | 2.05 / 31.23 | 112 |
| Activity metadata commit | 0.32 / 3.01 | 7.52 / 17.47 | 112 |
| Title edit commit | 3.61 / 13.89 | 3.37 / 7.24 | 112 |
| Append one stored message | 0.30 / 1.99 | 3.02 / 9.54 | 105 |
| Truncate history | 6.70 / 513.33 | 14.21 / 600.55 | 112 |

The empty session has no message to clone, hence 105 append samples. SQLite improves this workload's large-load tail, but activity commits, appends, and truncation medians are slower. Its `synchronous=FULL` commits have stronger crash durability than the existing file writes, which do not fsync. Do not interpret this as an equal-durability write comparison.

### Footprint and acceptance

Initial logical footprint was 3.376 GiB for files versus 3.296 GiB for SQLite (database, WAL, and shared memory included). After mutation/retention preparation it was 3.031 GiB versus 3.260 GiB. Neither format was compressed. Differences reflect representation, file checkpoint retention, and database allocation, not a compression feature.

Final checks passed: 121 base session tests (3 ignored), 77 app-core session tests, 2 SQLite search/replay tests, and 4 SQLite TUI tests. The TUI checks cover imported order and titles, mixed legacy/native rows, database-only discovery, and JSON transcript export. No throwaway fixtures or private corpus are committed; the benchmark examples and regression tests are retained for reproduction. This is not a claim that the entire workspace test suite passes.

Aggregate machine-readable evidence is in `benchmarks/session-storage-2026-10-27.json` and `benchmarks/session-picker-2026-10-27.json`. Earlier runs with mismatched picker ordering or titles were rejected, not used for speedup claims. The original live session directory, daemon, and default backend were not changed.
