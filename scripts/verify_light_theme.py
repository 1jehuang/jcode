#!/usr/bin/env python3
"""Verify light-theme detection end to end.

Spawns the freshly built jcode TUI in a PTY, replies to terminal queries
(OSC 10/11 foreground/background, DA, cursor position) advertising either a
white or black background, captures the rendered output, and compares the SGR
color usage. On the white background run, jcode should detect a light theme
and emit dark foreground colors; on the black run it should keep the normal
light-on-dark palette.
"""

import os
import fcntl
import pty
import re
import struct
import termios
import select
import subprocess
import sys
import time

JCODE = os.path.expanduser("~/.jcode/builds/current/jcode")

SGR_FG_RGB = re.compile(rb"38;2;(\d+);(\d+);(\d+)[;m]")
SGR_FG_256 = re.compile(rb"\x1b\[38;5;(\d+)m")


def reply_queries(master_fd: int, buffer: bytes, bg: str) -> bytes:
    # bg is "ffff" (white) or "0000" (black) per channel
    fg = b"0000" if bg == "ffff" else b"ffff"
    bgb = bg.encode()
    replies = [
        (b"\x1b[6n", b"\x1b[1;1R"),
        (b"\x1b[c", b"\x1b[?62;c"),
        (b"\x1b]10;?\x1b\\", b"\x1b]10;rgb:%s/%s/%s\x1b\\" % ((fg,) * 3)),
        (b"\x1b]11;?\x1b\\", b"\x1b]11;rgb:%s/%s/%s\x1b\\" % ((bgb,) * 3)),
        (b"\x1b]10;?\x07", b"\x1b]10;rgb:%s/%s/%s\x07" % ((fg,) * 3)),
        (b"\x1b]11;?\x07", b"\x1b]11;rgb:%s/%s/%s\x07" % ((bgb,) * 3)),
        (b"\x1b[14t", b"\x1b[4;600;800t"),
        (b"\x1b[16t", b"\x1b[6;16;8t"),
        (b"\x1b[18t", b"\x1b[8;24;80t"),
    ]
    # Reply in the order queries appear in the stream: terminals process
    # escape sequences sequentially, and colorsaurus relies on the OSC
    # responses arriving before the DA1 response.
    while True:
        first = None
        for query, response in replies:
            idx = buffer.find(query)
            if idx != -1 and (first is None or idx < first[0]):
                first = (idx, query, response)
        if first is None:
            return buffer
        _, query, response = first
        os.write(master_fd, response)
        buffer = buffer.replace(query, b"", 1)


def run_capture(bg: str, seconds: float = 12.0) -> bytes:
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 24, 80, 800, 600))
    env = dict(os.environ)
    env["TERM"] = "xterm-256color"
    env["COLORTERM"] = "truecolor"
    for k in ("JCODE_THEME", "TERM_PROGRAM", "TERM_PROGRAM_VERSION", "TMUX", "STY", "SSH_TTY", "SSH_CONNECTION", "SSH_CLIENT", "KITTY_WINDOW_ID", "GHOSTTY_RESOURCES_DIR", "WEZTERM_PANE", "ZELLIJ"):
        env.pop(k, None)
    proc = subprocess.Popen(
        [JCODE],
        stdin=slave,
        stdout=slave,
        stderr=slave,
        env=env,
        close_fds=True,
    )
    os.close(slave)
    out = b""
    buf = b""
    deadline = time.time() + seconds
    while time.time() < deadline:
        r, _, _ = select.select([master], [], [], 0.2)
        if master in r:
            try:
                chunk = os.read(master, 65536)
            except OSError:
                break
            if not chunk:
                break
            out += chunk
            buf += chunk
            buf = reply_queries(master, buf, bg)
    try:
        os.write(master, b"\x03\x03")  # ctrl-c twice to quit
        time.sleep(0.5)
    except OSError:
        pass
    proc.terminate()
    try:
        proc.wait(timeout=3)
    except subprocess.TimeoutExpired:
        proc.kill()
    os.close(master)
    return out


def fg_colors(raw: bytes) -> set[tuple[int, int, int]]:
    return {(int(r), int(g), int(b)) for r, g, b in SGR_FG_RGB.findall(raw)}


def lum(c: tuple[int, int, int]) -> float:
    r, g, b = c
    return (0.299 * r + 0.587 * g + 0.114 * b) / 255.0


def main() -> int:
    print("capturing with black background reply (expect dark theme)...")
    dark_raw = run_capture("0000")
    print("capturing with white background reply (expect light theme)...")
    light_raw = run_capture("ffff")

    dark = fg_colors(dark_raw)
    light = fg_colors(light_raw)
    print(f"dark-bg run:  {len(dark)} distinct truecolor fg colors")
    print(f"light-bg run: {len(light)} distinct truecolor fg colors")

    if len(dark) < 3 or len(light) < 3:
        print("FAIL: not enough SGR samples captured; TUI may not have rendered")
        return 1

    # Simple inversion used to turn dim rgb(80,80,80) into unreadable pale
    # rgb(175,175,175). Contrast repair now yields rgb(99,99,99) on the default
    # light surface. Dark mode must retain its original palette.
    if (80, 80, 80) not in dark:
        print("FAIL: dark run missing native dim color (80,80,80)")
        return 1
    if (99, 99, 99) not in light:
        print(f"FAIL: light run missing readable dim color (99,99,99); got {sorted(light)}")
        return 1
    if (80, 80, 80) in light:
        print("FAIL: light run still emits native dim color (80,80,80)")
        return 1

    # Bright accent colors must darken: the light run should contain colors that
    # are darker than anything in the dark run's brighter half.
    darkest_light = min(map(lum, light))
    if darkest_light > 0.25:
        print(f"FAIL: light run has no dark accents (min luminance {darkest_light:.2f})")
        return 1

    print("PASS: white background detected; muted light-theme text has readable contrast")
    return 0


if __name__ == "__main__":
    sys.exit(main())
