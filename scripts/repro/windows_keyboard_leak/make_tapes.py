"""Generate the VHS tapes for the keyboard-leak A/B.

The tape has to carry a real ESC byte: VHS's lexer keeps quoted strings byte for
byte (no escape processing in `readString`), and `ExecuteType` iterates runes and
presses each one, so an ESC inside the quotes becomes an actual key press. That
is what makes a tape able to emit the kitty CSI-u report instead of the literal
text `\\x1b[119;1:3u`.
"""

from __future__ import annotations

import os
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
HARNESS = ROOT / "harness"
TAPES = Path(os.environ.get("JCODE_REPRO_TAPES", ROOT / "tapes"))
ESC = "\x1b"
REPORT = f"{ESC}[119;1:3u"   # 'w' release, the shape that leaked into the composer


def tape(name: str, binary: Path, home: Path, title: str) -> str:
    # No nested quotes: the VHS lexer ends the argument at the next quote, and
    # none of these paths contain spaces.
    world = f'{HARNESS / "resume_world.cmd"} {home} {binary}'
    return "\n".join([
        f'Output "{TAPES / (name + ".gif")}"',
        f'Output "{TAPES / (name + ".mp4")}"',
        'Set Shell "cmd"',
        "Set FontSize 13",
        "Set Width 1400",
        "Set Height 860",
        "Set Padding 10",
        "Set Margin 0",
        "Set TypingSpeed 60ms",
        "Set Framerate 24",
        "",
        "Hide",
        f'Type "{world}"',
        "Enter",
        "Show",
        "",
        "# Wait for the resumed transcript to settle into its composer.",
        "Sleep 16s",
        "",
        f"# {title}: deliver the report a kitty terminal would send for 'w'.",
        f'Type "{REPORT}"',
        "Sleep 3s",
        "",
        "# Positive control: ordinary typing must still land in the composer.",
        'Type "zzqx"',
        "Sleep 3s",
        f'Screenshot "{TAPES / (name + ".png")}"',
        "Sleep 1s",
        "",
    ])


def main() -> int:
    TAPES.mkdir(parents=True, exist_ok=True)
    fixed = os.environ.get("JCODE_REPRO_FIXED_BIN")
    control = os.environ.get("JCODE_REPRO_CONTROL_BIN")
    if not fixed or not control:
        print("set JCODE_REPRO_FIXED_BIN and JCODE_REPRO_CONTROL_BIN to the two binaries",
              file=sys.stderr)
        return 2
    plan = [
        ("ab-fixed", Path(fixed), Path(os.environ.get("JCODE_REPRO_RUNS", ROOT / "runs")) / "vhs-fixed",
         "Fixed build"),
        ("ab-control", Path(control), Path(os.environ.get("JCODE_REPRO_RUNS", ROOT / "runs")) / "vhs-control",
         "Pre-scrub build"),
    ]
    for name, binary, home, title in plan:
        path = TAPES / f"{name}.tape"
        path.write_text(tape(name, binary, home, title), encoding="utf-8")
        print(f"{path}  ({path.stat().st_size} bytes, ESC={path.read_text(encoding='utf-8').count(ESC)})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
