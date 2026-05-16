#!/usr/bin/env python3
"""Read-only status summary for Jcode overnight autonomous runs."""

from __future__ import annotations

import json
import os
import subprocess
import sys
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Iterable


@dataclass
class RepoStatus:
    path: Path
    ok: bool
    branch_line: str
    dirty_lines: list[str]
    error: str | None = None


def run(cmd: list[str], cwd: Path) -> tuple[int, str, str]:
    proc = subprocess.run(cmd, cwd=cwd, text=True, capture_output=True, check=False)
    return proc.returncode, proc.stdout.strip(), proc.stderr.strip()


def git_status(path: Path) -> RepoStatus:
    code, out, err = run(["git", "status", "--short", "--branch"], path)
    if code != 0:
        return RepoStatus(path=path, ok=False, branch_line="", dirty_lines=[], error=err or out)
    lines = out.splitlines()
    return RepoStatus(
        path=path,
        ok=True,
        branch_line=lines[0] if lines else "",
        dirty_lines=lines[1:],
    )


def read_current_run(root: Path) -> dict[str, str | int | None]:
    current = root / ".jcode" / "overnight-runs" / "current"
    status_path = current / "status.md"
    result: dict[str, str | int | None] = {
        "path": str(status_path),
        "exists": "false",
        "last_checkpoint": None,
        "checkpoint_count": 0,
    }
    if not status_path.exists():
        return result
    text = status_path.read_text(errors="replace")
    checkpoints = [line for line in text.splitlines() if line.startswith("- 20")]
    result["exists"] = "true"
    result["checkpoint_count"] = len(checkpoints)
    result["last_checkpoint"] = checkpoints[-1] if checkpoints else None
    return result


def load_pr_watches(root: Path) -> list[dict[str, object]]:
    watch_dir = root / ".jcode" / "pr-feedback-watch"
    watches: list[dict[str, object]] = []
    if not watch_dir.exists():
        return watches
    for path in sorted(watch_dir.glob("*-state.json")):
        try:
            data = json.loads(path.read_text(errors="replace"))
        except Exception as exc:  # pragma: no cover - defensive CLI summary
            watches.append({"path": str(path), "error": str(exc)})
            continue
        polling = data.get("polling", {}) if isinstance(data, dict) else {}
        last_cycle = data.get("last_cycle", {}) if isinstance(data, dict) else {}
        pr = data.get("pr", {}) if isinstance(data, dict) else {}
        watches.append(
            {
                "watch_id": data.get("watch_id"),
                "repo": pr.get("repo"),
                "pr": pr.get("number"),
                "status": last_cycle.get("status"),
                "quiet_cycles": polling.get("quiet_cycles"),
                "required_quiet_cycles": polling.get("required_quiet_cycles"),
                "next_poll_at": polling.get("next_poll_at"),
                "actionable_count": len(data.get("pending_actionable", []) or []),
                "path": str(path),
            }
        )
    return watches


def print_repo(status: RepoStatus) -> None:
    print(f"repo: {status.path}")
    if not status.ok:
        print(f"  error: {status.error}")
        return
    print(f"  {status.branch_line}")
    print(f"  dirty_entries: {len(status.dirty_lines)}")
    for line in status.dirty_lines[:12]:
        print(f"  {line}")
    if len(status.dirty_lines) > 12:
        print(f"  ... {len(status.dirty_lines) - 12} more")


def main(argv: Iterable[str]) -> int:
    args = list(argv)
    root = Path(args[0]).resolve() if args else Path.cwd().resolve()
    target = Path(os.environ.get("JCODE_OVERNIGHT_TARGET", "/home/shawn/Documents/projects/constructive-toys-headless"))

    print(f"overnight_status_utc: {datetime.now(timezone.utc).isoformat()}")
    print(f"root: {root}")
    print()

    run_state = read_current_run(root)
    print("run_record:")
    for key, value in run_state.items():
        print(f"  {key}: {value}")
    print()

    print("git:")
    print_repo(git_status(root))
    if target.exists():
        print_repo(git_status(target))
    else:
        print(f"repo: {target}\n  missing")
    print()

    watches = load_pr_watches(root)
    print(f"pr_watches: {len(watches)}")
    for watch in watches[-10:]:
        print(
            "  - "
            f"{watch.get('watch_id')} repo={watch.get('repo')} pr={watch.get('pr')} "
            f"status={watch.get('status')} quiet={watch.get('quiet_cycles')}/{watch.get('required_quiet_cycles')} "
            f"actionable={watch.get('actionable_count')} next={watch.get('next_poll_at')}"
        )

    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
