#!/usr/bin/env python3
"""Precise GC for cargo incremental/fingerprint/deps caches.

Cargo never removes cache entries whose unit variant has died (feature-set,
flag, toolchain or dependency changes mint a fresh <crate>-<hash> directory
and orphan the old one). A day of iteration leaves gigabytes behind.

This script asks cargo for the set of unit hashes that are still reachable
(`cargo build --message-format=json` reports artifact filenames for every
unit in the plan, including fresh ones), then deletes everything else:

  target/<profile>/deps/<name>-<hex>.*          dead if hex not in plan
  target/<profile>/.fingerprint/<crate>-<hex>/  dead if hex not in plan
  target/<profile>/incremental/<crate>-<meta>/s-<us>-<rand>-<dg>/
      dead if its embedded microsecond timestamp does not pair with the
      dep-* mtime of any live fingerprint dir (same compile event)

The incremental dir hash cannot be joined to cargo's metadata hash by name,
so liveness is established through compile-event timestamps: rustc names
each work-product dir s-<base36 micros>-<rand>-<dep_graph>, and cargo writes
the unit's dep-* fingerprint file when the same compile finishes. Observed
skew is under 30 ms; the tolerance defaults to 600 s so even multi-minute
crate compiles pair safely. False negatives keep garbage (cheap); the only
false-positive risk is deleting a cache, which costs one recompile at most.

Usage:
    scripts/gc_incremental.py [--repo PATH] [--profile NAME]
                              [--plan "ARGS"]... [--tolerance SEC]
                              [--apply] [-v]

    --profile   target dir name under target/ (default: selfdev).
    --plan      cargo build argument set used as a liveness oracle, e.g.
                --plan "--profile selfdev -p jcode --bin jcode".
                Repeatable: live sets are UNIONED, which is required when
                different invocations resolve different feature sets for the
                same crate (each resolution mints a distinct unit variant).
                Default: a single `cargo build --profile <NAME>` plan.
                ("debug" as profile maps to `cargo build` with no flag.)
    --apply     actually delete; default is a dry run that reports sizes.

IMPORTANT: the oracle commands must cover every build invocation whose
caches you want to keep. `cargo build` and `cargo build -p jcode --bin
jcode` produce different unit variants of the same crates when feature
unification differs — an oracle covering only one will reap the other's
caches and force a rebuild. Units that only exist under `cargo test` /
`cargo check` are never covered by a build plan; their caches get reaped
and simply recompile on the next test run.
"""

import argparse
import datetime
import glob
import json
import os
import re
import shlex
import shutil
import subprocess
import sys

HEX16 = re.compile(r"-([0-9a-f]{16})(?=[/.]|$)")
HEX16_END = re.compile(r"-([0-9a-f]{16})$")


def log(msg, quiet):
    if not quiet:
        print(msg)


def collect_live_hexes(repo, profile, plans, quiet):
    """Run (usually no-op) oracle builds; return the union of live unit hashes."""
    target_dir = os.environ.get("CARGO_TARGET_DIR") or os.path.join(repo, "target")
    live_hex = set()
    root_artifacts = []
    for plan in plans:
        cmd = ["cargo", "build"] + shlex.split(plan) + ["--message-format=json"]
        log(f"$ {' '.join(cmd)}", quiet)
        proc = subprocess.run(cmd, cwd=repo, capture_output=True, text=True)
        if proc.returncode != 0:
            sys.stderr.write(proc.stdout)
            sys.stderr.write(proc.stderr)
            raise SystemExit(f"error: oracle build failed (rc={proc.returncode}); nothing deleted")
        for line in proc.stdout.splitlines():
            try:
                msg = json.loads(line)
            except json.JSONDecodeError:
                continue
            if msg.get("reason") != "compiler-artifact":
                continue
            for fname in msg.get("filenames", []):
                hexes = HEX16.findall(fname)
                if hexes:
                    live_hex.update(hexes)
                elif "/deps/" not in fname:
                    root_artifacts.append(fname)

    # Final artifacts (target/<profile>/jcode etc.) carry no hash in their
    # name; cargo hardlinks them to deps/<name>-<hex>. Resolve via inode.
    inos = set()
    for f in root_artifacts:
        try:
            inos.add(os.stat(f).st_ino)
        except OSError:
            pass
    if inos:
        for f in glob.glob(os.path.join(target_dir, profile, "deps", "*")):
            m = HEX16_END.search(os.path.basename(f))
            if not m:
                continue
            try:
                if os.stat(f).st_ino in inos:
                    live_hex.add(m.group(1))
            except OSError:
                pass
    return live_hex, target_dir


def size_of(path):
    total = 0
    if os.path.isfile(path):
        try:
            return os.path.getsize(path)
        except OSError:
            return 0
    for root, _, files in os.walk(path):
        for name in files:
            try:
                total += os.path.getsize(os.path.join(root, name))
            except OSError:
                pass
    return total


def rm(path, apply):
    if apply:
        if os.path.isdir(path):
            shutil.rmtree(path, ignore_errors=True)
        else:
            try:
                os.unlink(path)
            except OSError:
                pass


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--repo", default=".", help="workspace root (default: cwd)")
    ap.add_argument("--profile", default="selfdev", help="target dir profile name (default: selfdev)")
    ap.add_argument("--plan", action="append", default=None,
                    help='oracle build args, e.g. --plan "--profile selfdev -p jcode --bin jcode" (repeatable)')
    ap.add_argument("--tolerance", type=float, default=600.0, help="compile-event pairing tolerance in seconds (default: 600)")
    ap.add_argument("--apply", action="store_true", help="delete instead of dry-run")
    ap.add_argument("-v", "--verbose", action="store_true")
    ap.add_argument("-q", "--quiet", action="store_true")
    args = ap.parse_args()

    repo = os.path.abspath(args.repo)
    quiet = args.quiet
    if args.plan:
        plans = args.plan
    elif args.profile == "debug":
        plans = [""]
    else:
        plans = [f"--profile {args.profile}"]
    live_hex, target_dir = collect_live_hexes(repo, args.profile, plans, quiet)
    log(f"live unit hashes: {len(live_hex)}", quiet)

    prof = os.path.join(target_dir, args.profile)
    if not os.path.isdir(prof):
        raise SystemExit(f"error: {prof} does not exist")

    # --- live compile events: dep-* mtimes of live fingerprint dirs ---
    live_events = []
    dead_fp = []
    for d in glob.glob(os.path.join(prof, ".fingerprint", "*/")):
        unit_hex = os.path.basename(d.rstrip("/")).rsplit("-", 1)[-1]
        if unit_hex in live_hex:
            for f in glob.glob(os.path.join(d, "dep-*")):
                try:
                    live_events.append(os.stat(f).st_mtime)
                except OSError:
                    pass
        else:
            dead_fp.append(d.rstrip("/"))

    # --- incremental sweep ---
    dead_s = []          # dead s-* dirs inside crate dirs that stay alive
    dead_crates = []     # crate dirs with zero live s-* (delete whole dir)
    for crate_dir in glob.glob(os.path.join(prof, "incremental", "*/")):
        crate_dir = crate_dir.rstrip("/")
        has_live = False
        saw_any = False
        dead_here = []
        for s_dir in glob.glob(os.path.join(crate_dir, "s-*-*/")):
            saw_any = True
            s_dir = s_dir.rstrip("/")
            parts = os.path.basename(s_dir).split("-")
            try:
                ts = int(parts[1], 36) / 1e6
            except (IndexError, ValueError):
                has_live = True  # unparsable: keep
                continue
            if any(abs(ts - ev) < args.tolerance for ev in live_events):
                has_live = True
            else:
                dead_here.append(s_dir)
        if saw_any and not has_live:
            dead_crates.append(crate_dir)
        else:
            dead_s.extend(dead_here)

    dead_deps = []
    deps_dir = os.path.join(prof, "deps")
    if os.path.isdir(deps_dir):
        for f in glob.glob(os.path.join(deps_dir, "*")):
            m = HEX16.search(os.path.basename(f))
            if m and m.group(1) not in live_hex:
                dead_deps.append(f)

    s_mb = sum(size_of(d) for d in dead_s) / 1e6
    crate_mb = sum(size_of(d) for d in dead_crates) / 1e6
    fp_mb = sum(size_of(d) for d in dead_fp) / 1e6
    deps_mb = sum(size_of(f) for f in dead_deps) / 1e6

    if args.verbose:
        for d in dead_s:
            print("  dead s-*:", d)
        for d in dead_crates:
            print("  dead crate dir:", d)
        for d in dead_fp:
            print("  dead fp:", d)
        for f in dead_deps:
            print("  dead dep:", f)

    print(f"incremental s-* dirs dead: {len(dead_s)} ({s_mb:.0f} MB)")
    print(f"incremental crate dirs fully dead: {len(dead_crates)} ({crate_mb:.0f} MB)")
    print(f"fingerprint dirs dead: {len(dead_fp)} ({fp_mb:.0f} MB)")
    print(f"deps files dead: {len(dead_deps)} ({deps_mb:.0f} MB)")
    total = s_mb + crate_mb + fp_mb + deps_mb
    print(f"total reclaimable: {total:.0f} MB" + ("" if args.apply else "  (dry run; pass --apply to delete)"))

    if not args.apply:
        return

    for s_dir in dead_s:
        rm(s_dir, True)
        lock = s_dir.rsplit("-", 1)[0] + ".lock"
        if os.path.exists(lock):
            rm(lock, True)
    for d in dead_crates:
        rm(d, True)
    for d in dead_fp:
        rm(d, True)
    for f in dead_deps:
        rm(f, True)
    # crate dirs left with only .lock files / emptied
    for crate_dir in glob.glob(os.path.join(prof, "incremental", "*/")):
        crate_dir = crate_dir.rstrip("/")
        if not glob.glob(os.path.join(crate_dir, "s-*-*/")):
            rm(crate_dir, True)
    log("done", quiet)


if __name__ == "__main__":
    main()
