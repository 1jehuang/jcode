#!/usr/bin/env python3

"""Run a command with a hard timeout and readable diagnostics."""

from __future__ import annotations

import os
import signal
import subprocess
import sys
from typing import Sequence


def _usage() -> int:
    print(
        "usage: run_with_timeout.py <timeout-seconds> <command> [args...]",
        file=sys.stderr,
    )
    return 2


def _kill_process_group(proc: subprocess.Popen[bytes]) -> None:
    try:
        pgid = os.getpgid(proc.pid)
    except ProcessLookupError:
        return
    try:
        os.killpg(pgid, signal.SIGTERM)
    except ProcessLookupError:
        return


def main(argv: Sequence[str]) -> int:
    if len(argv) < 3:
        return _usage()

    try:
        timeout_seconds = int(argv[1])
    except ValueError:
        print(f"invalid timeout: {argv[1]!r}", file=sys.stderr)
        return 2

    command = list(argv[2:])
    print(f"==> Running with timeout {timeout_seconds}s: {' '.join(command)}")
    proc = subprocess.Popen(command, start_new_session=True)
    try:
        return proc.wait(timeout=timeout_seconds)
    except subprocess.TimeoutExpired:
        print(
            f"::error::Command exceeded timeout after {timeout_seconds}s: {' '.join(command)}",
            file=sys.stderr,
        )
        _kill_process_group(proc)
        try:
            proc.wait(timeout=30)
            return 124
        except subprocess.TimeoutExpired:
            try:
                os.killpg(os.getpgid(proc.pid), signal.SIGKILL)
            except ProcessLookupError:
                pass
            return 124


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
