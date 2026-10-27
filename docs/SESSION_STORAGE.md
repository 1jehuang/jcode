# Session storage backends

## Scope

This change separates session persistence from the agent's in-memory `Session` model. The existing JSON snapshot plus JSONL journal remains the default. The SQLite backend is opt-in. Neither the benchmark nor the migration removes source sessions.

The storage boundary owns session discovery, metadata summaries, loading, and committing a session's changed state. It does not own provider calls, context compaction, rendering, process presence, credentials, or global configuration. A future backend must preserve the same session semantics, including replacement and truncation, not only appends.

## Compatibility

File import/export remains an interchange format, not a requirement to mirror each SQLite write to a JSON snapshot. Older binaries cannot read SQLite-only sessions. Retain an export before reverting a session to a file-only binary. Metadata listing must not deserialize message bodies. Session commits must update metadata and changed transcript rows atomically.

Any caller that still enumerates the `sessions` directory directly must be identified before treating SQLite as a replacement for every workflow. Default behavior must remain unchanged for users who do not opt in.

## Benchmark contract

The comparison uses the same captured session corpus and public storage operations for both implementations. Only aggregate results may be published. Do not commit transcript content, session identifiers, prompts, working directories, credentials, or the private corpus.

Report separately:

- Source import time and peak memory. Import is not a steady-state read.
- First-process and repeated metadata listing, at equal result limits.
- Full resume loads for the same sessions, stratified by serialized size.
- Metadata-only commits, one-message append commits, and history replacement/truncation.
- Reopen and read-back validation after commits.
- Allocated and logical storage including SQLite database, WAL, and shared-memory files, and file snapshots plus journals.
- Durability settings and transaction boundaries. Do not compare a buffered append with a durable commit without identifying the difference.

Use repeated, alternating backend order and report medians and upper-percentile latency. A new process is not a cold filesystem cache. Do not clear the host's global page cache or interrupt other workloads. Report warm-cache results honestly and keep import/index construction out of query-only latency.

The acceptance check is equality of loaded sessions and mutations after reopening, not merely successful SQL execution. A speedup in listing does not establish a speedup in full resume, write throughput, search, memory use, or recovery.
