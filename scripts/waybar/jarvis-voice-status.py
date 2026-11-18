#!/usr/bin/env python3
"""Waybar JSON stream for Jcode's actual capture meter. Never opens a microphone."""
import argparse
import html
import json
import math
import os
from pathlib import Path
import stat
import time

BARS = "▁▂▃▄▅▆▇█"


def runtime_dir():
    return Path(os.environ.get("XDG_RUNTIME_DIR", f"/run/user/{os.getuid()}")) / "jcode-jarvis"


def safe_directory(path):
    try:
        info = path.lstat()
        return stat.S_ISDIR(info.st_mode) and info.st_uid == os.getuid() and not info.st_mode & 0o077
    except OSError:
        return False


def latest_status(directory, now=None):
    now = time.time() if now is None else now
    if not safe_directory(directory):
        return None
    candidates = []
    for path in directory.glob("*.json"):
        try:
            info = path.lstat()
            if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_size > 8192:
                continue
            if not 0 <= now - info.st_mtime <= 3:
                continue
            value = json.loads(path.read_text())
            if not isinstance(value, dict):
                continue
            if value.get("pid") != int(path.stem) or not isinstance(value.get("capturing"), bool):
                continue
            os.kill(value["pid"], 0)
            candidates.append((info.st_mtime, path, value))
        except (OSError, ValueError, TypeError, json.JSONDecodeError):
            continue
    return max(candidates, default=None, key=lambda item: item[0])


def render(value):
    if value is None:
        return {"text": "", "tooltip": "Jarvis microphone is off", "class": "off"}
    state = value.get("state", "unknown")
    capturing = value.get("capturing") is True
    levels = value.get("levels", [])
    levels = levels[-12:] if isinstance(levels, list) else []
    levels = [float(v) if isinstance(v, (int, float)) and math.isfinite(v) else 0.0 for v in levels]
    waveform = "".join(BARS[max(0, min(7, int(v * 8)))] for v in levels).ljust(12, BARS[0])
    text = f"🎙 {waveform}" if capturing else "🎙 paused" if state == "paused" else "🎙 off"
    counters = value.get("counters", {})
    counters = counters if isinstance(counters, dict) else {}
    tooltip = f"Jarvis: {state}\nMicrophone capture: {'on' if capturing else 'off'}\nClick to pause/resume.\n"
    for key, number in counters.items():
        if isinstance(number, (int, float)) and math.isfinite(number):
            tooltip += f"{key}: {number}\n"
    return {"text": text, "tooltip": html.escape(tooltip), "class": "capturing" if capturing else "paused" if state == "paused" else "off"}


def toggle(directory):
    status = latest_status(directory)
    if not status:
        return
    path = status[1].with_suffix(".paused")
    try:
        if path.exists():
            path.unlink()
        else:
            fd = os.open(path, os.O_CREAT | os.O_EXCL | os.O_WRONLY | os.O_NOFOLLOW, 0o600)
            os.close(fd)
    except OSError:
        pass


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--toggle", action="store_true")
    parser.add_argument("--once", action="store_true")
    args = parser.parse_args()
    directory = runtime_dir()
    if args.toggle:
        toggle(directory)
        return
    previous = None
    while True:
        found = latest_status(directory)
        output = json.dumps(render(found[2] if found else None), ensure_ascii=False)
        if output != previous or args.once:
            print(output, flush=True)
            previous = output
        if args.once:
            break
        time.sleep(0.2)


if __name__ == "__main__":
    try:
        main()
    except (BrokenPipeError, KeyboardInterrupt):
        pass
