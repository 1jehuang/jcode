#!/usr/bin/env python3
"""Rehearse the real jcode release-update flow in a throwaway sandbox.

Runs the genuine updater code path (check -> download -> checksum -> install ->
channel symlinks -> graceful client reload -> shared-server catch-up) against a
fake "latest release" served from localhost. Nothing outside the sandbox is
touched: JCODE_HOME, the runtime/socket dir, and the launcher dir all live under
the sandbox, and the release-URL override is only honored by jcode when
JCODE_HOME is sandboxed.

Every run wipes the sandbox, so the experience is exactly repeatable.

Usage (normally launched by `/update-rehearsal` or Alt+Shift+U in self-dev):
  scripts/update_rehearsal.py                  # default: auto-update on launch
  scripts/update_rehearsal.py --rate 5         # slow link (MiB/s)
  scripts/update_rehearsal.py --drop-at 40     # connection drops at 40%, resumes
  scripts/update_rehearsal.py --bad-checksum   # checksum mismatch failure
  scripts/update_rehearsal.py --manual         # no auto-update; type /update
  scripts/update_rehearsal.py --up-to-date     # release is not newer
"""

from __future__ import annotations

import argparse
import hashlib
import http.server
import io
import json
import os
import shutil
import signal
import socket
import subprocess
import sys
import tarfile
import threading
import time
from pathlib import Path

ASSET_STEM = {
    ("linux", "x86_64"): "jcode-linux-x86_64",
    ("linux", "aarch64"): "jcode-linux-aarch64",
    ("darwin", "x86_64"): "jcode-macos-x86_64",
    ("darwin", "arm64"): "jcode-macos-aarch64",
    ("darwin", "aarch64"): "jcode-macos-aarch64",
}

REPO_DIR = Path(__file__).resolve().parent.parent
MIB = 1024 * 1024


def log(msg: str) -> None:
    print(f"\x1b[36m[update-rehearsal]\x1b[0m {msg}", flush=True)


def asset_stem() -> str:
    key = (sys.platform if sys.platform != "linux" else "linux", os.uname().machine)
    if key not in ASSET_STEM:
        sys.exit(f"unsupported platform for rehearsal: {key}")
    return ASSET_STEM[key]


def default_root() -> Path:
    base = os.environ.get("XDG_CACHE_HOME") or str(Path.home() / ".cache")
    return Path(base) / "jcode-update-rehearsal"


def default_binary() -> Path:
    candidates = [REPO_DIR / "target" / "selfdev" / "jcode", REPO_DIR / "target" / "release" / "jcode"]
    existing = [c for c in candidates if c.exists()]
    if not existing:
        sys.exit("no built jcode binary found (expected target/selfdev/jcode); run a selfdev build first")
    return max(existing, key=lambda p: p.stat().st_mtime)


def cargo_version() -> str:
    for line in (REPO_DIR / "Cargo.toml").read_text().splitlines():
        if line.startswith("version"):
            return line.split("=", 1)[1].strip().strip('"')
    return "0.1.0"


def bump(version: str, patch_delta: int) -> str:
    major, minor, patch = (int(x) for x in version.split(".")[:3])
    return f"{major}.{minor}.{max(0, patch + patch_delta)}"


def git_short_hash() -> str:
    try:
        return subprocess.check_output(["git", "rev-parse", "--short", "HEAD"], cwd=REPO_DIR, text=True).strip()
    except Exception:
        return "rehearsal"


def wrapper_script(version: str, payload_name: str) -> str:
    # Mirrors the fast-release wrapper produced by scripts/quick-release.sh.
    return f"""#!/usr/bin/env sh
set -eu
export JCODE_RUNTIME_RELEASE_SEMVER="{version}"
export JCODE_RUNTIME_RELEASE_GIT_HASH="{git_short_hash()}"
export JCODE_RUNTIME_RELEASE_GIT_DATE="rehearsal"
export JCODE_RUNTIME_RELEASE_GIT_TAG="v{version}"
self=$0
while [ -L "$self" ]; do
    link=$(readlink -- "$self")
    case $link in
        /*) self=$link ;;
        *) self=$(dirname -- "$self")/$link ;;
    esac
done
self_dir=$(CDPATH= cd -- "$(dirname -- "$self")" && pwd)
exec "$self_dir/{payload_name}" "$@"
"""


def link_or_copy(src: Path, dst: Path) -> None:
    dst.parent.mkdir(parents=True, exist_ok=True)
    if dst.exists():
        dst.unlink()
    try:
        os.link(src, dst)
    except OSError:
        shutil.copy2(src, dst)


def prepare_payload(binary: Path, cache_dir: Path, strip: bool) -> Path:
    """Snapshot (and strip, like real releases) the binary once per build."""
    st = binary.stat()
    key = f"{st.st_size}-{int(st.st_mtime)}-{'s' if strip else 'u'}"
    payload = cache_dir / f"payload-{key}.bin"
    if payload.exists():
        return payload
    for stale in cache_dir.glob("payload-*.bin"):
        stale.unlink()
    for stale in cache_dir.glob("release-*.tar.gz"):
        stale.unlink()
    cache_dir.mkdir(parents=True, exist_ok=True)
    tmp = payload.with_suffix(".tmp")
    log(f"snapshotting {binary} ({st.st_size / MIB:.0f} MiB)" + (" and stripping" if strip else ""))
    shutil.copy2(binary, tmp)
    if strip and shutil.which("strip"):
        subprocess.run(["strip", "--strip-unneeded", str(tmp)], check=False)
    tmp.chmod(0o755)
    tmp.rename(payload)
    return payload


def build_release_tarball(payload: Path, version: str, stem: str, cache_dir: Path) -> Path:
    wrapper = wrapper_script(version, f"{stem}.bin").encode()
    tag = hashlib.sha256(wrapper).hexdigest()[:8]
    tarball = cache_dir / f"release-{payload.stem}-{version}-{tag}.tar.gz"
    if tarball.exists():
        return tarball
    for stale in cache_dir.glob(f"release-{payload.stem}-{version}-*"):
        stale.unlink()
    log(f"packaging fake release v{version} (one-time per build, gzip -1)")
    tmp = tarball.with_suffix(".tmp")
    with open(tmp, "wb") as raw:
        gz = subprocess.Popen(["gzip", "-1", "-c"], stdin=subprocess.PIPE, stdout=raw)
        with tarfile.open(fileobj=gz.stdin, mode="w|") as tar:
            info = tarfile.TarInfo(stem)
            info.size = len(wrapper)
            info.mode = 0o755
            info.mtime = int(time.time())
            tar.addfile(info, io.BytesIO(wrapper))
            tar.add(payload, arcname=f"{stem}.bin")
        gz.stdin.close()
        if gz.wait() != 0:
            sys.exit("gzip failed while packaging fake release")
    tmp.rename(tarball)
    return tarball


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(4 * MIB), b""):
            h.update(chunk)
    return h.hexdigest()


def install_old_version(home: Path, payload: Path, version: str, stem: str) -> Path:
    builds = home / "builds"
    vdir = builds / "versions" / version
    vdir.mkdir(parents=True)
    link_or_copy(payload, vdir / f"{stem}.bin")
    wrapper = vdir / "jcode"
    wrapper.write_text(wrapper_script(version, f"{stem}.bin"))
    wrapper.chmod(0o755)
    for channel in ("stable", "current", "shared-server"):
        (builds / channel).mkdir(parents=True)
        (builds / channel / "jcode").symlink_to(wrapper)
        (builds / f"{channel}-version").write_text(version)
    (home / "bin").mkdir()
    (home / "bin" / "jcode").symlink_to(builds / "current" / "jcode")
    return home / "bin" / "jcode"


def write_sandbox_config(home: Path, manual: bool) -> None:
    (home / "config.toml").write_text(
        "[features]\n"
        f"check_updates = {'false' if manual else 'true'}\n"
        'update_channel = "stable"\n'
    )
    # Looks like an established install so first-run onboarding never hijacks
    # the screen; the rehearsal is about the update UX, not onboarding.
    (home / "setup_hints.json").write_text(
        json.dumps(
            {
                "launch_count": 100,
                "hotkey_dismissed": True,
                "alacritty_dismissed": True,
                "startup_spawn_hint_dismissed": True,
                "mac_ghostty_dismissed": True,
            }
        )
    )


class Scenario:
    def __init__(self, args: argparse.Namespace, tarball: Path, checksum: str, stem: str, new_version: str):
        self.args = args
        self.tarball = tarball
        self.size = tarball.stat().st_size
        self.checksum = checksum
        self.stem = stem
        self.new_version = new_version
        self.dropped = False
        self.lock = threading.Lock()
        self.base_url = ""

    def release_json(self) -> bytes:
        asset_name = f"{self.stem}.tar.gz"
        return json.dumps(
            {
                "tag_name": f"v{self.new_version}",
                "name": f"v{self.new_version} (rehearsal)",
                "html_url": "https://example.invalid/rehearsal",
                "published_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
                "target_commitish": "main",
                "assets": [
                    {"name": asset_name, "browser_download_url": f"{self.base_url}/{asset_name}", "size": self.size},
                    {"name": "SHA256SUMS", "browser_download_url": f"{self.base_url}/SHA256SUMS", "size": 0},
                ],
            }
        ).encode()

    def sums(self) -> bytes:
        digest = "0" * 64 if self.args.bad_checksum else self.checksum
        return f"{digest}  {self.stem}.tar.gz\n".encode()


def make_handler(sc: Scenario):
    class Handler(http.server.BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, fmt, *a):  # keep the TUI terminal clean
            with open(sc.args.server_log, "a") as f:
                f.write(f"{time.strftime('%H:%M:%S')} {self.address_string()} {fmt % a}\n")

        def send_bytes(self, body: bytes, ctype: str = "application/json") -> None:
            self.send_response(200)
            self.send_header("Content-Type", ctype)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def do_GET(self):  # noqa: N802
            if self.path == "/release.json":
                if sc.args.check_delay:
                    time.sleep(sc.args.check_delay)
                return self.send_bytes(sc.release_json())
            if self.path == "/SHA256SUMS":
                return self.send_bytes(sc.sums(), "text/plain")
            if self.path == f"/{sc.stem}.tar.gz":
                return self.serve_asset()
            self.send_error(404)

        def serve_asset(self):
            start = 0
            rng = self.headers.get("Range")
            if rng and rng.startswith("bytes="):
                start = int(rng[6:].split("-")[0] or 0)
            remaining = sc.size - start
            if start:
                self.send_response(206)
                self.send_header("Content-Range", f"bytes {start}-{sc.size - 1}/{sc.size}")
            else:
                self.send_response(200)
            self.send_header("Content-Type", "application/gzip")
            self.send_header("Content-Length", str(remaining))
            self.send_header("Accept-Ranges", "bytes")
            self.end_headers()

            rate = sc.args.rate * MIB if sc.args.rate > 0 else 0
            drop_at = None
            with sc.lock:
                if sc.args.drop_at is not None and not sc.dropped:
                    drop_at = int(sc.size * sc.args.drop_at / 100)
            chunk = 256 * 1024
            sent_total = start
            began = time.monotonic()
            sent_here = 0
            with open(sc.tarball, "rb") as f:
                f.seek(start)
                while True:
                    data = f.read(chunk)
                    if not data:
                        break
                    if drop_at is not None and sent_total + len(data) >= drop_at:
                        with sc.lock:
                            sc.dropped = True
                        self.log_message("dropping connection at %d bytes (simulated)", sent_total)
                        if sc.args.drop_stall:
                            time.sleep(sc.args.drop_stall)
                        self.close_connection = True
                        try:
                            self.connection.shutdown(socket.SHUT_RDWR)
                        except OSError:
                            pass
                        return
                    try:
                        self.wfile.write(data)
                    except (BrokenPipeError, ConnectionResetError):
                        return
                    sent_total += len(data)
                    sent_here += len(data)
                    if rate:
                        ahead = sent_here / rate - (time.monotonic() - began)
                        if ahead > 0:
                            time.sleep(ahead)

    return Handler


def sandbox_pids(home: Path) -> list[int]:
    """Processes whose environment points at this sandbox's JCODE_HOME."""
    needle = f"JCODE_HOME={home}".encode()
    pids = []
    proc = Path("/proc")
    if not proc.exists():
        return pids
    for entry in proc.iterdir():
        if not entry.name.isdigit() or int(entry.name) == os.getpid():
            continue
        try:
            env = (entry / "environ").read_bytes()
        except OSError:
            continue
        if needle in env.split(b"\0"):
            pids.append(int(entry.name))
    return pids


def kill_sandbox_processes(home: Path) -> None:
    pids = sandbox_pids(home)
    for sig in (signal.SIGTERM, signal.SIGKILL):
        for pid in pids:
            try:
                os.kill(pid, sig)
            except ProcessLookupError:
                pass
        if not pids:
            return
        deadline = time.monotonic() + 3
        while time.monotonic() < deadline and any(Path(f"/proc/{p}").exists() for p in pids):
            time.sleep(0.1)
        pids = [p for p in pids if Path(f"/proc/{p}").exists()]


def scrubbed_env(home: Path, run_dir: Path, release_url: str) -> dict[str, str]:
    env = {
        k: v
        for k, v in os.environ.items()
        if not k.startswith("JCODE_") and k not in ("XDG_RUNTIME_DIR",)
    }
    env.update(
        {
            "JCODE_HOME": str(home),
            "JCODE_RUNTIME_DIR": str(run_dir),
            "JCODE_INSTALL_DIR": str(home / "bin"),
            "JCODE_UPDATE_RELEASE_URL": release_url,
            "JCODE_NO_TELEMETRY": "1",
            "PATH": f"{home / 'bin'}:{env.get('PATH', '')}",
        }
    )
    return env


def summarize(home: Path, old: str, new: str) -> None:
    builds = home / "builds"
    print()
    log("result:")
    for channel in ("stable", "current", "shared-server"):
        marker = builds / f"{channel}-version"
        value = marker.read_text().strip() if marker.exists() else "?"
        status = "unchanged" if value == old else ("updated" if value == new else value)
        print(f"    {channel:<14} {value:<10} {status}")
    meta = home / "update_metadata.json"
    if meta.exists():
        data = json.loads(meta.read_text())
        secs = data.get("last_release_update_secs")
        if secs:
            print(f"    install duration recorded: {secs:.1f}s")
    print(f"    logs: {home / 'logs'}")


def run_version(launcher: Path, env: dict[str, str]) -> str:
    out = subprocess.run([str(launcher), "--version"], env=env, capture_output=True, text=True, timeout=60)
    return (out.stdout or out.stderr).strip().splitlines()[0] if (out.stdout or out.stderr).strip() else ""


def headless_check(launcher, env, workspace, home, old, new, server, args) -> int:
    try:
        before = run_version(launcher, env)
        log(f"before: {before}")
        started = time.monotonic()
        result = subprocess.run(
            [str(launcher), "update", "--no-selfdev"],
            cwd=workspace,
            env=env,
            stdin=subprocess.DEVNULL,
            capture_output=True,
            text=True,
            timeout=600,
        )
        elapsed = time.monotonic() - started
        tail = (result.stdout + result.stderr).strip().splitlines()[-8:]
        for line in tail:
            print(f"    | {line}")
        after = run_version(launcher, env)
        log(f"after:  {after}  (update took {elapsed:.1f}s, exit {result.returncode})")
    finally:
        server.shutdown()
        kill_sandbox_processes(home)
    summarize(home, old, new)
    if args.server_log.exists():
        drops = [l for l in args.server_log.read_text().splitlines() if "dropping" in l or "206" in l]
        for line in drops:
            print(f"    server: {line}")

    expect_new = not (args.bad_checksum or args.up_to_date)
    current = (home / "builds" / "current-version").read_text().strip()
    ok = (f"v{new}" in after and current == new) if expect_new else (f"v{old}" in after and current == old)
    log("PASS" if ok else "FAIL")
    return 0 if ok else 1


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--binary", type=Path, help="jcode binary to package (default: newest target/selfdev or release)")
    p.add_argument("--root", type=Path, default=default_root(), help="sandbox root (wiped every run)")
    p.add_argument("--from-version", help="version the sandbox starts on (default: Cargo version)")
    p.add_argument("--to-version", help="version of the fake release (default: from + 1 patch)")
    p.add_argument("--rate", type=float, default=10.0, help="download speed in MiB/s, 0 = unlimited (default 10)")
    p.add_argument("--drop-at", type=float, metavar="PCT", help="drop the connection once at PCT%% to exercise resume")
    p.add_argument("--drop-stall", type=float, default=0.0, metavar="SECS", help="hang SECS before the drop")
    p.add_argument("--check-delay", type=float, default=0.0, metavar="SECS", help="delay the release-check response")
    p.add_argument("--bad-checksum", action="store_true", help="serve a mismatching SHA256SUMS")
    p.add_argument("--up-to-date", action="store_true", help="serve a release that is not newer")
    p.add_argument("--manual", action="store_true", help="disable auto-update; trigger it yourself with /update")
    p.add_argument("--no-strip", action="store_true", help="package the binary unstripped (bigger, slower)")
    p.add_argument("--keep", action="store_true", help="leave sandbox jcode processes running after exit")
    p.add_argument("--prepare-only", action="store_true", help="build the sandbox and fake release, then exit")
    p.add_argument(
        "--headless-check",
        action="store_true",
        help="no TUI: run `jcode update` in the sandbox and verify the install (CI/smoke test)",
    )
    p.add_argument("jcode_args", nargs=argparse.REMAINDER, help="extra args passed to jcode after --")
    args = p.parse_args()

    if sys.platform != "linux":
        log("note: process cleanup relies on /proc and may be incomplete on this OS")

    stem = asset_stem()
    binary = (args.binary or default_binary()).resolve()
    old = args.from_version or cargo_version()
    new = args.to_version or (old if args.up_to_date else bump(old, 1))

    root = args.root.resolve()
    cache_dir = root / "cache"
    home = root / "home"
    run_dir = root / "run"
    workspace = root / "workspace"
    args.server_log = root / "server.log"

    if home.exists():
        kill_sandbox_processes(home)
    for d in (home, run_dir, workspace):
        shutil.rmtree(d, ignore_errors=True)
    if args.server_log.exists():
        args.server_log.unlink()
    for d in (cache_dir, home, run_dir, workspace):
        d.mkdir(parents=True, exist_ok=True)
    run_dir.chmod(0o700)

    payload = prepare_payload(binary, cache_dir, strip=not args.no_strip)
    tarball = build_release_tarball(payload, new, stem, cache_dir)
    checksum_cache = tarball.with_suffix(".sha256")
    if checksum_cache.exists():
        checksum = checksum_cache.read_text().strip()
    else:
        checksum = sha256_file(tarball)
        checksum_cache.write_text(checksum)

    launcher = install_old_version(home, payload, old, stem)
    write_sandbox_config(home, args.manual)

    sc = Scenario(args, tarball, checksum, stem, new)
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), make_handler(sc))
    sc.base_url = f"http://127.0.0.1:{server.server_address[1]}"
    threading.Thread(target=server.serve_forever, daemon=True).start()
    release_url = f"{sc.base_url}/release.json"

    log(f"sandbox: {root}")
    log(f"installed v{old}; fake release v{new} ({sc.size / MIB:.0f} MiB) at {release_url}")
    scenario = []
    if args.rate:
        scenario.append(f"{args.rate:g} MiB/s")
    if args.drop_at is not None:
        scenario.append(f"drop at {args.drop_at:g}%")
    if args.bad_checksum:
        scenario.append("bad checksum")
    if args.up_to_date:
        scenario.append("up to date")
    if args.manual:
        scenario.append("manual: type /update")
    log("scenario: " + (", ".join(scenario) or "default"))

    env = scrubbed_env(home, run_dir, release_url)
    if args.prepare_only:
        log(f"prepared. run: env JCODE_HOME={home} JCODE_RUNTIME_DIR={run_dir} {launcher}")
        server.shutdown()
        return 0

    if args.headless_check:
        return headless_check(launcher, env, workspace, home, old, new, server, args)

    extra = [a for a in args.jcode_args if a != "--"]
    cmd = [str(launcher), "--no-selfdev", *extra]
    log("launching: " + " ".join(cmd))
    time.sleep(0.6)
    try:
        code = subprocess.call(cmd, cwd=workspace, env=env)
    except KeyboardInterrupt:
        code = 130
    finally:
        server.shutdown()
        if not args.keep:
            kill_sandbox_processes(home)
    summarize(home, old, new)
    if sys.stdin.isatty():
        try:
            input("\n[update-rehearsal] press Enter to close ")
        except (EOFError, KeyboardInterrupt):
            pass
    return code


if __name__ == "__main__":
    sys.exit(main())
