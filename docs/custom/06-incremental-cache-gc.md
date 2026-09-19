# [06] Precise incremental build-cache GC

## What it does

`scripts/gc_incremental.py` deletes dead cargo cache entries under
`target/<profile>/` while preserving every cache the current build graph can
still reuse. It reclaims space that the previous 7-day mtime rule could not
touch precisely: ~2.4 GB was found on a profile built only hours earlier.

## Why

Cargo never garbage-collects cache entries whose unit *variant* has died.
Feature flags, dep-version bumps, toolchain or target changes mint a fresh
`<crate>-<hash>` directory and orphan the old one forever; a day of
iteration on this workspace leaves tens of GB behind. mtime-based pruning
cannot tell a dormant-but-valid cache from a permanently dead one — a crate
that simply was not recompiled recently looks identical to a dead variant.

## How liveness is decided

1. `cargo build --profile <P> --message-format=json` is a near no-op right
   after a build and reports artifact filenames for **every** unit in the
   plan (including fresh units). The `<name>-<hex>` hashes extracted from
   those paths are the live set. Final artifacts like `target/<P>/jcode`
   carry no hash in their filename; cargo hardlinks them to
   `deps/<name>-<hex>`, so their hash is recovered by inode match.
2. `.fingerprint/<crate>-<hex>` and `deps/<name>-<hex>.*` are live iff their
   hex is in the set (fingerprint dirs and deps filenames share cargo's
   unit hash).
3. `incremental/<crate>-<meta>/s-<a>-<b>-<depgraph>` uses a rustc-internal
   name that cannot be joined to cargo's hash. Liveness is instead
   established through the compile event itself: `<a>` is a base36
   microsecond timestamp of when rustc created the dir, and the unit's
   `dep-*` fingerprint file is written when that same compile finishes.
   An `s-*` dir is live iff its timestamp pairs with a live fingerprint's
   `dep-*` mtime within `--tolerance` (default 600 s; observed skew is
   0-30 ms). Crate dirs left with no live `s-*` are deleted entirely.

Failure modes are one-directional: a missed pairing keeps garbage (cheap),
a wrong deletion only costs one recompile — never correctness.

## Usage

```bash
scripts/gc_incremental.py --profile selfdev                       # dry run
scripts/gc_incremental.py --profile selfdev --apply               # delete
scripts/gc_incremental.py --profile selfdev --apply \
    --plan "--profile selfdev -p jcode --bin jcode" \
    --plan "--profile selfdev"                                    # unioned oracles
```

Run it right after a build so the oracle builds are no-ops (~0.5 s each).
`--profile` names the target dir. Each `--plan` is a cargo build argument
set; the script runs `cargo build <ARGS> --message-format=json` per plan
and **unions** their live unit sets. `CARGO_TARGET_DIR` is honored.
Deleting is off unless `--apply` is passed; a failed oracle build aborts
without touching anything.

**The oracle plans must cover every build invocation whose caches you want
to keep.** `cargo build` and `cargo build -p jcode --bin jcode` resolve
different feature sets for the same crates and mint distinct unit variants;
an oracle covering only one will reap the other's caches and force a
rebuild (observed: a workspace-only oracle deleted the `-p` variants and
the next `-p` build recompiled ~7 min). Union every plan you actually run.

## Caveats

- The live set is the **build** plan. Units that exist only under
  `cargo test`/`cargo check` are outside it; their caches get reaped and
  recompile on the next test run. For this fork's workflow (selfdev build
  driven, occasional targeted `cargo test`) that trade-off is fine.
- `.fingerprint`/`deps` sweeping is exact; incremental sweeping is exact up
  to the pairing tolerance, which errs toward keeping.
- `target/<profile>/build/` output dirs are not swept.

## Files

- `scripts/gc_incremental.py` — the tool (stdlib-only Python 3).

## Upstream status

Not upstreamable as a patch — it is a standalone maintainer-side tool and
does not modify jcode source. Could be published independently if ever
useful.
