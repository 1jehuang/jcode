#!/usr/bin/env python3
"""Execute only the exact uploaded preview ARM archive on a native ARM runner."""
import os
from pathlib import Path
import platform
import re
import struct
import subprocess
import sys
import tarfile
import tempfile


MEMBER = "jcode-linux-aarch64"


def verify(archive, version):
    if platform.system() != "Linux" or platform.machine() not in ("aarch64", "arm64"):
        raise ValueError("ARM runtime acceptance requires native ARM64 Linux")
    if not re.fullmatch(r"v1\.0\.0-preview\.[1-9][0-9]*", version):
        raise ValueError("expected an explicit preview version identity")
    # Do not extract archive paths, links or unexpected companion files. The ARM
    # contract is exactly one executable, unlike the x86 wrapper/payload archive.
    with tarfile.open(archive, "r:gz") as package:
        members = package.getmembers()
        if (len(members) != 1 or members[0].name != MEMBER
                or not members[0].isfile() or not members[0].mode & 0o111):
            raise ValueError("expected exactly one executable ARM archive member")
        with package.extractfile(members[0]) as source:
            binary = source.read()
    if (len(binary) < 64 or binary[:7] != b"\x7fELF\x02\x01\x01"
            or struct.unpack_from("<HH", binary, 16) not in ((2, 183), (3, 183))):
        raise ValueError("expected an AArch64 ELF64 executable")
    with tempfile.TemporaryDirectory(prefix="preview-arm-runtime-") as directory:
        executable = Path(directory) / MEMBER
        executable.write_bytes(binary)
        executable.chmod(0o755)
        args = [str(executable), "--no-update", "--no-selfdev"]
        result = subprocess.run(args + ["--version"], check=True, text=True,
                                capture_output=True, timeout=60)
        print(result.stdout, end="")
        # Bound both ends so preview.12 cannot accept preview.123 or .12-extra.
        if not re.search(r"(?<![\w.-])v?" + re.escape(version.lstrip("v"))
                         + r"(?![\w.-])", result.stdout):
            raise ValueError("uploaded ARM archive has the wrong preview version")
        for command in ("receive", "credentials", "router-credentials", "cancel-return"):
            subprocess.run(args + ["cloud", command, "--help"], check=True,
                           stdout=subprocess.DEVNULL, timeout=60)
    print("Native ARM runtime acceptance passed for uploaded archive " + str(archive))


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit("usage: verify_preview_arm_runtime.py ARCHIVE")
    verify(sys.argv[1], os.environ["JCODE_BUILD_SEMVER"])
