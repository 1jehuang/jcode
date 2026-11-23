#!/usr/bin/env python3
"""Waybar JSON stream for Jcode's actual capture meter. Never opens a microphone."""
import argparse
import html
import json
import math
import os
from pathlib import Path
import re
import stat
import subprocess
import sys
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


def retry_token(value):
    token = value.get("retry_token")
    if value.get("retry_supported") is True and isinstance(token, str) and re.fullmatch(r"[0-9a-f]{16}", token):
        return token
    return None


LEGACY_ERROR = "Open the Jarvis client for the error and recovery instructions. This client does not support Waybar retry."


def safe_line(value, default):
    if not isinstance(value, str):
        return default
    return " ".join("".join(ch if ch.isprintable() else " " for ch in value[:180]).split()) or default


def render(value):
    if value is None:
        return {"text": "", "tooltip": "Jarvis microphone is off", "class": "off"}
    state = value.get("state", "unknown")
    capturing = value.get("capturing") is True
    if state == "error":
        lines = ["Jarvis microphone error", f"Microphone capture: {'on' if capturing else 'off'}"]
        if retry_token(value):
            lines.extend([safe_line(value.get("error_reason"), "Capture stopped."),
                          safe_line(value.get("error_hint"), "Check the Jarvis client for details."),
                          "After resolving the issue, click to request a retry or use /voice in Jarvis."])
        else:
            lines.append(LEGACY_ERROR)
        return {"text": "🎙 !", "tooltip": html.escape("\n".join(lines)), "class": "error"}
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
    if state == "triggered":
        return {"text": "✓ Jarvis", "tooltip": html.escape("Voice request accepted\n" + tooltip), "class": "triggered"}
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
    if status[2].get("state") == "error":
        token = retry_token(status[2])
        if not token:
            return LEGACY_ERROR
        path = status[1].with_suffix(f".retry-{token}")
        try:
            fd = os.open(path, os.O_CREAT | os.O_EXCL | os.O_WRONLY | os.O_NOFOLLOW, 0o600)
            os.close(fd)
        except FileExistsError:
            pass  # Repeated clicks coalesce; never toggle a pending retry off.
        except OSError:
            return "Could not request a retry. Use /voice in the Jarvis client."
        return None
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
        message = toggle(directory)
        if message:
            print(message, file=sys.stderr)
            try:
                subprocess.run(["notify-send", "Jarvis microphone", message], timeout=2, check=False)
            except (OSError, subprocess.TimeoutExpired):
                pass
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
