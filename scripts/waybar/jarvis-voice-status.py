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


def native_render(value):
    """Bounded measured envelope for Waybar's native Cairo waveform module."""
    legacy = render(value)
    legacy["tooltip"] = html.unescape(legacy["tooltip"])  # Native GTK uses plain text.
    if value is None:
        return {"capturing": False, "state": "off", "levels": [],
                "sequence": 0, "sample_interval_ms": 50, **legacy}
    levels = value.get("levels", [])
    levels = levels[-128:] if isinstance(levels, list) else []
    levels = [max(0.0, min(1.0, float(v)))
              if isinstance(v, (int, float)) and not isinstance(v, bool)
              and math.isfinite(v) else 0.0 for v in levels]
    interval = value.get("sample_interval_ms", 200)  # Original clients published at 5 Hz.
    interval = interval if isinstance(interval, int) and not isinstance(interval, bool) else 200
    sequence = value.get("sequence", 0)
    sequence = sequence if isinstance(sequence, int) and not isinstance(sequence, bool) else 0
    state = value.get("state", "unknown")
    state = state[:80] if isinstance(state, str) else "unknown"
    return {**legacy, "text": "", "capturing": value.get("capturing") is True,
            "state": state, "levels": levels, "pid": value.get("pid", 0),
            "sequence": max(0, min(2**64 - 1, sequence)),
            "sample_interval_ms": max(20, min(1000, interval))}


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
    parser.add_argument("--native", action="store_true", help="emit measured samples for the native Waybar waveform module")
    args = parser.parse_args()
    directory = runtime_dir()
    if args.toggle:
        toggle(directory)
        return
    previous = None
    while True:
        found = latest_status(directory)
        value = dict(found[2]) if found else None
        if value is not None:
            value.setdefault("sequence", int(found[0] * 1_000_000_000))
        output = json.dumps(native_render(value) if args.native else render(value), ensure_ascii=False, allow_nan=False)
        if output != previous or args.once:
            print(output, flush=True)
            previous = output
        if args.once:
            break
        time.sleep(0.05 if args.native and value and value.get("capturing") is True else 0.2)


if __name__ == "__main__":
    try:
        main()
    except (BrokenPipeError, KeyboardInterrupt):
        pass
