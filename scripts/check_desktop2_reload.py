#!/usr/bin/env python3
"""Verify desktop2 activates a worker without changing its native window.

The check snapshots every matching niri window, runs a caller-supplied build or
reload command, and continuously requires the exact window ID, host PID,
workspace, geometry, and focus state to remain unchanged. It also requires the
stable host's instance marker to report a different worker generation, proving
that changed callback code ran inside the existing host.

Example:
    scripts/check_desktop2_reload.py -- selfdev build --target desktop2
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import subprocess
import sys
import threading
import time
from dataclasses import dataclass
@dataclass(frozen=True)
class Window:
    id: int
    pid: int
    title: str
    workspace_id: int | None
    is_focused: bool
    layout: str


def niri_windows(title_fragment: str, pid_filter: int | None = None) -> tuple[Window, ...]:
    result = subprocess.run(
        ["niri", "msg", "-j", "windows"],
        check=True,
        capture_output=True,
        text=True,
        timeout=2,
    )
    fragment = title_fragment.casefold()
    windows = []
    for raw in json.loads(result.stdout):
        title = raw.get("title") or ""
        pid = raw.get("pid")
        window_id = raw.get("id")
        if (
            fragment in title.casefold()
            and isinstance(pid, int)
            and isinstance(window_id, int)
            and (pid_filter is None or pid == pid_filter)
        ):
            windows.append(
                Window(
                    id=window_id,
                    pid=pid,
                    title=title,
                    workspace_id=raw.get("workspace_id"),
                    is_focused=bool(raw.get("is_focused")),
                    # Relative column ordinals shift when unrelated neighboring
                    # windows open or close. Geometry is the tile/window size and
                    # offset owned by this window, not pos_in_scrolling_layout.
                    layout=json.dumps(
                        {
                            key: (raw.get("layout") or {}).get(key)
                            for key in (
                                "tile_size",
                                "window_size",
                                "tile_pos_in_workspace_view",
                                "window_offset_in_tile",
                            )
                        },
                        sort_keys=True,
                    ),
                )
            )
    return tuple(sorted(windows, key=lambda window: window.id))


def activation_markers(windows: tuple[Window, ...]) -> dict[int, str | None]:
    root = Path.home() / ".jcode" / "selfdev" / "desktop2-instances"
    markers = {}
    for window in windows:
        try:
            markers[window.pid] = (root / str(window.pid)).read_text()
        except OSError:
            markers[window.pid] = None
    return markers


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--title",
        default="jcode desktop2",
        help="case-insensitive title fragment identifying desktop2 windows",
    )
    parser.add_argument("--poll-ms", type=float, default=20.0)
    parser.add_argument("--pid", type=int, help="check only this stable host PID")
    parser.add_argument(
        "--settle-seconds",
        type=float,
        default=3.0,
        help="how long the unchanged host must remain stable after the command",
    )
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not command:
        parser.error("provide the reload command after --")
    if args.poll_ms <= 0 or args.settle_seconds < 0:
        parser.error("poll interval must be positive and settle time non-negative")

    try:
        initial = niri_windows(args.title, args.pid)
    except (OSError, subprocess.SubprocessError, json.JSONDecodeError) as error:
        print(f"FAIL: cannot query niri windows: {error}", file=sys.stderr)
        return 2
    if not initial:
        print(f"FAIL: no window title contains {args.title!r}", file=sys.stderr)
        return 2
    before_markers = activation_markers(initial)

    start = time.monotonic()
    stop = threading.Event()
    samples: list[tuple[float, tuple[Window, ...]]] = []
    observer_error: list[BaseException] = []

    def observe() -> None:
        while not stop.is_set():
            try:
                windows = niri_windows(args.title, args.pid)
                samples.append((time.monotonic() - start, windows))
                if windows != initial:
                    stop.set()
                    return
            except BaseException as error:  # Preserve observer failures for the main thread.
                observer_error.append(error)
                stop.set()
                return
            stop.wait(args.poll_ms / 1000.0)

    observer = threading.Thread(target=observe, name="desktop2-window-observer", daemon=True)
    observer.start()
    identities = [(window.id, window.pid) for window in initial]
    print(f"watching stable desktop2 window(s) {identities}")
    try:
        completed = subprocess.run(command, check=False)
        deadline = time.monotonic() + args.settle_seconds
        while time.monotonic() < deadline and not stop.is_set():
            time.sleep(min(0.05, deadline - time.monotonic()))
    finally:
        stop.set()
        observer.join(timeout=3)

    if observer_error:
        print(f"FAIL: compositor observation failed: {observer_error[0]}", file=sys.stderr)
        return 2
    changed = next(((elapsed, windows) for elapsed, windows in samples if windows != initial), None)
    if changed is not None:
        elapsed, windows = changed
        print(
            f"FAIL: native window changed at +{elapsed:.3f}s\n"
            f"  before={initial!r}\n  after={windows!r}",
            file=sys.stderr,
        )
        return 1
    if completed.returncode != 0:
        print(f"FAIL: reload command exited {completed.returncode}", file=sys.stderr)
        return completed.returncode or 1

    after_markers = activation_markers(initial)
    unchanged = [
        pid
        for pid, marker in after_markers.items()
        if marker is None or marker == before_markers.get(pid) or "worker_build=" not in marker
    ]
    if unchanged:
        print(
            f"FAIL: host(s) {unchanged} did not report execution of a new worker; "
            f"before={before_markers!r} after={after_markers!r}",
            file=sys.stderr,
        )
        return 1

    print(
        f"PASS: {len(initial)} exact native window(s) remained unchanged across "
        f"{len(samples)} samples and activated new worker code"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
