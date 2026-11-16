#!/usr/bin/env python3
"""Same-run handoff integrity checks, not a cryptographic build attestation."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import struct
import subprocess

TOOLCHAIN = "nightly-2026-10-08"
JUSTRUST_REV = "a7e7d02cbd50358d1aea056d3c17e5496586d62a"
CONTRACTS = {
    "linux-arm64": {
        "target": "aarch64-unknown-linux-gnu", "producer": "hosted-linux-x86_64",
        "command": ["build", "--remote", "--locked", "--release", "--bin", "jcode",
                    "--target", "aarch64-unknown-linux-gnu", "-Zbuild-std=std,panic_abort",
                    "--features", "linux-compat-vendored-openssl"],
        "features": ["default", "linux-compat-vendored-openssl"],
        "target_rustflags": "-Ztls-model=emulated",
    },
    "macos-arm64": {
        "target": "aarch64-apple-darwin", "producer": "native-macos-arm64",
        "command": ["build", "--local", "--locked", "--release", "--bin", "jcode",
                    "--target", "aarch64-apple-darwin"],
        "features": ["default"],
        "target_rustflags": "",
    },
}


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def regular(path):
    if path.is_symlink() or not path.is_file():
        raise ValueError(f"not a regular file: {path}")


def architecture(path, lane):
    regular(path)
    data = path.read_bytes()[:64]
    if lane == "linux-arm64":
        if (len(data) < 64 or data[:6] != b"\x7fELF\x02\x01"
                or struct.unpack_from("<H", data, 18)[0] != 183
                or struct.unpack_from("<H", data, 16)[0] not in (2, 3)):
            raise ValueError("expected little-endian AArch64 ELF64 executable")
    elif (len(data) < 32 or data[:4] != b"\xcf\xfa\xed\xfe"
          or struct.unpack_from("<I", data, 4)[0] != 0x0100000C
          or struct.unpack_from("<I", data, 12)[0] != 2):
        raise ValueError("expected thin ARM64 Mach-O executable")


def context():
    sha = os.environ["GITHUB_SHA"]
    if not re.fullmatch(r"[0-9a-f]{40}", sha):
        raise ValueError("expected exact source SHA")
    if os.environ["GITHUB_REPOSITORY"] != "1jehuang/jcode" or os.environ["GITHUB_REF"] != "refs/heads/master":
        raise ValueError("only canonical master dispatches are trusted")
    return {key: os.environ[key] for key in
            ("GITHUB_SHA", "GITHUB_REPOSITORY", "GITHUB_REF", "GITHUB_RUN_ID", "GITHUB_RUN_ATTEMPT")}


def expected(lane):
    return {"schema": 1, "context": context(), "lane": lane,
            "toolchain": TOOLCHAIN, "justrust_revision": JUSTRUST_REV,
            "contract": CONTRACTS[lane], "profile_opt_level": "3",
            "lock_sha256": digest(Path("Cargo.lock")), "features": CONTRACTS[lane]["features"]}


def build_receipt(directory, lane, sealing=False):
    report_path = directory / "build-report.json"
    regular(report_path)
    report = json.loads(report_path.read_text())
    command = [arg for arg in CONTRACTS[lane]["command"] if arg not in ("--remote", "--local")]
    if report.get("exit") != 0 or report.get("args") != command:
        raise ValueError("build receipt does not match successful pinned command")
    hosted = lane == "linux-arm64"
    if bool(report.get("remote")) != hosted:
        raise ValueError("build receipt execution location mismatch")
    if sealing and hosted:
        run_id = report.get("id", "")
        if not re.fullmatch(r"[A-Za-z0-9_-]+", run_id):
            raise ValueError("invalid justrust run ID")
        home = Path(os.environ.get("JUSTRUST_HOME", str(Path.home() / ".justrust")))
        meta = json.loads((home / "runs" / run_id / "meta.json").read_text())
        if meta.get("place") != "hosted":
            raise ValueError("ARM build did not run on hosted infrastructure")
    return {"report_sha256": digest(report_path), "place": "hosted" if hosted else "native"}


def seal(directory, lane):
    binary = directory / "jcode"
    architecture(binary, lane)
    manifest = expected(lane)
    manifest["build_receipt"] = build_receipt(directory, lane, sealing=True)
    manifest["binary_sha256"] = digest(binary)
    path = directory / "provenance.json"
    path.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
    return digest(path)


def verify(directory, lane, manifest_digest):
    path = directory / "provenance.json"
    regular(path)
    if not re.fullmatch(r"[0-9a-f]{64}", manifest_digest) or digest(path) != manifest_digest:
        raise ValueError("manifest digest differs from producer job output")
    manifest = json.loads(path.read_text())
    binary = directory / "jcode"
    architecture(binary, lane)
    wanted = expected(lane)
    wanted["build_receipt"] = build_receipt(directory, lane)
    wanted["binary_sha256"] = digest(binary)
    if manifest != wanted:
        raise ValueError("binary or same-run source/build provenance mismatch")


def smoke(directory):
    binary = (directory / "jcode").resolve()
    binary.chmod(0o755)
    # No account credentials or inherited application config reaches the executable.
    import tempfile
    with tempfile.TemporaryDirectory() as home:
        env = {"PATH": os.defpath, "HOME": home, "XDG_CONFIG_HOME": home,
               "XDG_CACHE_HOME": home, "JCODE_NO_UPDATE": "1", "TERM": "dumb"}
        for args, marker in [(["--version"], "jcode"), (["--help"], "Usage:"),
                             (["run", "--help"], "Usage:")]:
            result = subprocess.run([str(binary), *args], env=env, cwd=home,
                                    capture_output=True, text=True, timeout=30, check=True)
            if marker not in result.stdout:
                raise ValueError(f"missing {marker!r} in smoke output for {args}")
            print(result.stdout)


def main():
    parser = argparse.ArgumentParser(__doc__)
    parser.add_argument("action", choices=["seal", "verify", "smoke"])
    parser.add_argument("--lane", choices=CONTRACTS, required=True)
    parser.add_argument("--directory", type=Path, default=Path("handoff"))
    parser.add_argument("--manifest-digest", default="")
    args = parser.parse_args()
    if args.action == "seal":
        print(seal(args.directory, args.lane))
    else:
        verify(args.directory, args.lane, args.manifest_digest)
        if args.action == "smoke":
            smoke(args.directory)


if __name__ == "__main__":
    main()
