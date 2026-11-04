# Cloud session preview cache

Cloud ownership and resume stay on the remote daemon. The resume picker keeps a
separate, read-only replica for previews in
`~/.jcode/cache/cloud-previews/v1/`. It never installs that replica in the
canonical local `sessions/` directory or changes a migration lease.

## Behavior

- A first preview fetches the snapshot, append journal, and optional remote
  telemetry over the existing bounded SSH transport, with negotiated SSH
  compression. The remote CLI needs no upgrade.
- A validated replica is written atomically with owner-only file permissions.
  Cache keys isolate the SSH host, session ID, and migration epoch.
- Subsequent selections and picker reopenings read the replica locally. A copy
  older than 15 seconds is still usable, including offline, while one background
  refresh per cache key runs in that process. This is refresh-on-use, not a
  background download of every remote session while the picker is closed.
- The next normal preview refresh can display the updated replica. Remote
  workspace probes remain separate and do not block the transcript.
- Failed fetches, malformed payloads, and session-ID mismatches never replace the
  last good copy. Missing or malformed optional telemetry does not prevent a
  usable transcript, and local telemetry is never substituted for remote facts.
- Individual blobs are bounded to 64 MiB. A 256 MiB aggregate budget evicts older
  hashed blobs after successful writes, preserving the newly written copy.

## Measurements

Measured on the user's Linux XPS against the real `swan` cloud conversation on
November 2, 2026 (local time), with about 47.5 MB of snapshot plus journal and
roughly 16,000 stored messages. These are representative samples, not a
statistical latency guarantee or a cold-disk benchmark.

| Case | Measured time |
| --- | --- |
| Original uncached cloud load | 15,011 ms, followed by a 15-second timeout on another attempt |
| Original message projection after load | 18 ms |
| Compressed first cloud load plus projection | 11,016 ms |
| Cached local load plus projection | 252 ms and 272 ms |
| Cached load, projection, and first 120x40 preview frame | 367 ms, 304 ms, 310 ms |
| Real 160x45 terminal, startup search input to visible cloud transcript | 1,168 ms and 1,279 ms |

The terminal acceptance check launched the deployed `820e7e913` build twice in
separate PTYs, opened the actual `--resume` picker, searched for the exact swan
session ID, and verified substantive transcript rows and the cloud host/icon in
the rendered screen. These end-to-end samples include session-list startup and
terminal I/O, unlike the narrower renderer benchmark. Both runs reused the
persistent local replica across processes and exited without resuming or
modifying the remote conversation.

The first-frame benchmark uses the real downloaded transcript and the real
picker renderer with Ratatui's test backend. It asserts that the cloud footer is
present and the loading placeholder is absent. It excludes the initial session
list scan, terminal I/O, and optional background workspace probes. The uncached
fetch can still be slow. The main improvement removes network waiting from
already-downloaded previews.

Recorded justrust runs:

- Baseline: `20261102-160723660-2286233` (the second fetch timed out).
- Compressed cold fetch and cached loads: `20261102-161228969-2309302`.
- Cached first-frame rendering: `20261102-161553632-2323876`.

The ignored live benchmark must run locally because it needs the user's local
SSH identity and managed cloud proxy credentials. It is not sent to hosted CI:

```bash
JCODE_CLOUD_PREVIEW_BENCH_SESSION=<session-id> \
JCODE_CLOUD_PREVIEW_BENCH_HOST=<ssh-host> \
justrust test --local -p jcode-tui --lib live_cloud_preview_benchmark \
  -- --ignored --nocapture --test-threads=1
```

Ordinary cache, transport, and chrome regression tests use synthetic data and
run with hosted `justrust test`.
