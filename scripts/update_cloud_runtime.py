#!/usr/bin/env python3
"""Stage a Linux cloud runtime without disturbing live sessions.

By default only installs an immutable, checksum-verified dependency bundle.
--activate-launcher atomically updates /opt/jcode-cloud/jcode after verification.
It NEVER restarts/reloads the daemon. Coordinate a state-preserving protocol
reload separately, after checking active sessions and the daemon's reload target.
"""
import argparse
import hashlib
import os
from pathlib import Path
import re
import shlex
import shutil
import subprocess
import tarfile
import tempfile
import time


def bundle(binary, directory):
    binary = Path(binary).resolve(strict=True)
    payload = Path(directory) / "payload"
    payload.mkdir()
    shutil.copy2(binary, payload / "jcode.bin")
    env = dict(os.environ)
    env.pop("LD_PRELOAD", None)
    env.pop("LD_LIBRARY_PATH", None)
    dependencies = subprocess.check_output(["ldd", str(binary)], env=env, text=True)
    loader = None
    for line in dependencies.splitlines():
        if "not found" in line:
            raise RuntimeError(f"missing dependency: {line}")
        paths = [word for word in line.split() if word.startswith("/")]
        if not paths:
            continue
        path = Path(paths[0])
        if not re.fullmatch(r"[A-Za-z0-9_.-]+", path.name):
            raise RuntimeError(f"unsupported dependency: {path}")
        shutil.copy2(path, payload / path.name)
        if path.name.startswith("ld-linux"):
            loader = path.name
    if loader is None:
        raise RuntimeError("binary must have a Linux dynamic loader")
    (payload / "jcode").write_text(
        '#!/bin/sh\nset -eu\np=$(readlink -f "$0")\nd=$(dirname "$p")\n'
        f'exec "$d/{loader}" --library-path "$d" "$p.bin" "$@"\n'
    )
    (payload / "jcode").chmod(0o755)
    archive = Path(directory) / "runtime.tar.gz"
    with tarfile.open(archive, "w:gz", compresslevel=1) as tar:
        for member in sorted(payload.iterdir()):
            tar.add(member, arcname=member.name, recursive=False)
    with archive.open("rb") as stream:
        checksum = hashlib.file_digest(stream, "sha256").hexdigest()
    return archive, checksum


def install_script(checksum, activate=False):
    if not re.fullmatch(r"[a-f0-9]{64}", checksum):
        raise ValueError("invalid SHA-256")
    script = r'''set -eu
umask 077
root="$HOME/.local/share/jcode/cloud-releases"
mkdir -p "$root"
upload=$(mktemp -d "$root/.upload.XXXXXX")
trap 'rm -rf "$upload"' EXIT
cat > "$upload/runtime.tar.gz"
printf '%s  %s\n' CHECKSUM "$upload/runtime.tar.gz" | sha256sum --check --status
release="$root/runtime-CHECKSUM"
mkdir "$upload/payload"
tar xzf "$upload/runtime.tar.gz" --no-same-owner --no-same-permissions -C "$upload/payload"
probe() {
  env -u JCODE_RUNTIME_RELEASE_SEMVER -u JCODE_RUNTIME_RELEASE_GIT_HASH \
    -u JCODE_RUNTIME_RELEASE_GIT_DATE -u JCODE_RUNTIME_RELEASE_GIT_TAG \
    "$1" --no-update --no-selfdev --version
}
probe "$upload/payload/jcode"
for command in receive credentials router-credentials cancel-return replace-receive group-protocol; do
  "$upload/payload/jcode" --no-update --no-selfdev cloud "$command" --help >/dev/null
done
# Serialize publication/activation. A stale lock requires manual inspection,
# never delete a lock that may belong to another live deployment.
lock="$root/.runtime-update-lock"
mkdir "$lock" || { echo 'another runtime update holds the lock' >&2; exit 1; }
trap 'rmdir "$lock"; rm -rf "$upload"' EXIT
if [ ! -e "$release" ]; then
  mv "$upload/payload" "$release"
else
  # Never silently reuse a modified release with the same archive identity.
  diff -qr "$upload/payload" "$release" >/dev/null
fi
probe "$release/jcode"
printf 'STAGED_RUNTIME=%s/jcode\n' "$release"
'''.replace("CHECKSUM", checksum)
    if activate:
        script += r'''
backend=/opt/jcode-cloud/jcode
[ -L "$backend" ] || { echo 'refusing to replace a non-symlink backend' >&2; exit 1; }
previous=$(readlink "$backend")
# Preserve the old target on disk for a coordinated rollback.
printf '%s\n' "$previous" > "$root/previous-backend-$(date +%s)-$$"
next="$backend.runtime-update.$$"
sudo -n ln -s "$release/jcode" "$next"
sudo -n mv -Tf "$next" "$backend"
if ! probe "$backend"; then
  sudo -n ln -s "$previous" "$next"
  sudo -n mv -Tf "$next" "$backend"
  echo 'launcher verification failed, restored previous backend' >&2
  exit 1
fi
printf 'ACTIVATED_LAUNCHER=%s\nPREVIOUS_BACKEND=%s\n' "$release/jcode" "$previous"
echo 'Daemon unchanged. Coordinate a state-preserving protocol reload separately.'
'''
    return script


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("--host", default="jcode-cloud")
    parser.add_argument("--ssh-config", type=Path, default=Path.home() / ".jcode/cloud/ssh_config")
    parser.add_argument("--activate-launcher", action="store_true",
                        help="explicitly switch backend symlink, NEVER reload daemon")
    args = parser.parse_args()
    scratch = Path(os.environ.get("JCODE_SCRATCH_DIR", Path.home() / ".jcode/scratch"))
    scratch.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="jcode-runtime-", dir=scratch) as directory:
        archive, checksum = bundle(args.binary, directory)
        command = ["ssh", "-T", "-o", "BatchMode=yes", "-o", "ForwardAgent=no",
                   "-F", str(args.ssh_config), "--", args.host,
                   "bash -c " + shlex.quote(install_script(checksum, args.activate_launcher))]
        # Staging is immutable and safe to repeat after a dropped SSH channel.
        # Do not retry explicit activation: a disconnect may hide a successful switch.
        attempts = 1 if args.activate_launcher else 3
        for attempt in range(attempts):
            with archive.open("rb") as stream:
                result = subprocess.run(command, stdin=stream)
            if result.returncode != 255 or attempt + 1 == attempts:
                result.check_returncode()
                break
            print("SSH dropped while staging, retrying verified bundle", flush=True)
            time.sleep(2 * (attempt + 1))


if __name__ == "__main__":
    main()
