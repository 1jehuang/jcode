"""Observability-first repro for the Windows keyboard-protocol leak (#224).

Plays the role of a *stale kitty-capable terminal*: it spawns a real jcode
binary inside a real ConPTY, watches the app's output, and then delivers
keystrokes the way a terminal whose progressive-enhancement flags were already
armed would deliver them, i.e. kitty CSI-u reports (`\\x1b[119;1:3u`) instead of
plain characters. That is exactly the input the console decoder cannot consume
and the input the decoder-side scrub is meant to strip.

Every run writes its own artifacts, so a red run is diagnosable without a
re-run:

    <run-dir>/pty-stream.txt     decoded PTY output (what the app painted)
    <run-dir>/screen/*.txt       pyte VT screen snapshots at each checkpoint
    <run-dir>/app-log.txt        the isolated home's logs, concatenated
    <run-dir>/fixture.sha256     hash of the session copy this run resumed
    <run-dir>/result.json        checks, timings, verdict

A run is green only when every check passes. `--dump-only` is a pilot mode that
prints the settled screen and exits, used to keep the readiness logic honest.
"""

from __future__ import annotations

import argparse
import glob
import hashlib
import json
import os
import shutil
import sys
import threading
import time
from pathlib import Path

import pyte
from winpty import PtyProcess

DEFAULT_BINARY = r"C:\Users\koosh\AppData\Local\jcode\bin\jcode.exe"

# The exact report shape observed leaking into the composer on Windows.
# 119 = 'w', 1 = no modifiers, 3 = key-release event type.
LEAK = "\x1b[119;1:3u"
LEAK_TEXT = "119;1:3"
POSITIVE = "zzqx"

# Timing contract of the scrub: a real leaked report arrives as one contiguous
# burst, and the scrub refuses fragments whose characters are more than 750ms
# apart. The harness types fast enough to be inside that contract and records
# the observed gap so a slow-machine red is distinguishable from a real red.
MAX_INTERCHAR_GAP_MS = 750.0


class Capture:
    """Decoded PTY stream plus a live VT screen."""

    def __init__(self, rows: int = 40, cols: int = 120) -> None:
        self.chunks: list[str] = []
        self.lock = threading.Lock()
        self.alive = True
        self.last_output_at = 0.0
        self.saw_output = False
        self.vt = pyte.Screen(cols, rows)
        self.stream = pyte.Stream(self.vt)

    def pump(self, proc: PtyProcess) -> None:
        while self.alive:
            try:
                chunk = proc.read(4096)
            except EOFError:
                break
            except Exception:
                break
            if not chunk:
                time.sleep(0.01)
                continue
            now = time.monotonic()
            with self.lock:
                self.chunks.append(chunk)
                self.last_output_at = now
                self.saw_output = True
                try:
                    self.stream.feed(chunk)
                except Exception:
                    pass

    def text(self) -> str:
        with self.lock:
            return "".join(self.chunks)

    def screen(self) -> str:
        with self.lock:
            return "\n".join(self.vt.display)

    def wait_quiescent(self, quiet_s: float, timeout_s: float) -> bool:
        deadline = time.monotonic() + timeout_s
        while time.monotonic() < deadline:
            with self.lock:
                quiet = self.saw_output and (time.monotonic() - self.last_output_at) >= quiet_s
            if quiet:
                return True
            time.sleep(0.05)
        return False


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def resume_name(fixture: Path) -> str:
    """The memorable session name a saved session encodes: session_<name>_<ts>_<id>."""
    parts = fixture.stem.split("_")
    return parts[1] if len(parts) >= 3 and parts[0] == "session" else fixture.stem


def prepare_run(run_dir: Path, fixture: Path) -> tuple[Path, Path]:
    """Fresh, byte-identical world for this run."""
    home = run_dir / "home"
    sessions = home / "sessions"
    cwd = run_dir / "cwd"
    # Reusing a run directory must not let an earlier run's log satisfy this one:
    # the menu verdict reads the log for the session load.
    logs = home / "logs"
    if logs.exists():
        shutil.rmtree(logs, ignore_errors=True)
    for path in (sessions, cwd):
        path.mkdir(parents=True, exist_ok=True)
    target = sessions / fixture.name
    shutil.copyfile(fixture, target)
    os.chmod(target, 0o644)
    return home, cwd


def collect_log(home: Path, out: Path) -> str:
    parts: list[str] = []
    for name in sorted(glob.glob(str(home / "logs" / "**" / "*"), recursive=True)):
        path = Path(name)
        if not path.is_file():
            continue
        try:
            parts.append(f"### {path}\n" + path.read_text(encoding="utf-8", errors="replace"))
        except Exception as exc:  # keep going; a missing log is evidence too
            parts.append(f"### {path}\n<unreadable: {exc}>\n")
    text = "\n".join(parts)
    out.write_text(text, encoding="utf-8")
    return text


def wait_for_text(cap: "Capture", needle: str, timeout_s: float) -> bool:
    """Poll the VT screen for a marker. The session picker animates, so waiting for
    output to go quiet there never returns; a visible marker is the honest signal."""
    deadline = time.monotonic() + timeout_s
    while time.monotonic() < deadline:
        if needle in cap.screen():
            return True
        time.sleep(0.05)
    return False


def wait_for_text_gone(cap: "Capture", needle: str, timeout_s: float) -> bool:
    deadline = time.monotonic() + timeout_s
    while time.monotonic() < deadline:
        if needle not in cap.screen():
            return True
        time.sleep(0.05)
    return False


def main() -> int:
    try:
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    except Exception:
        pass

    ap = argparse.ArgumentParser()
    ap.add_argument("--binary", default=DEFAULT_BINARY)
    ap.add_argument("--label", default="unlabelled")
    ap.add_argument("--run-dir", required=True)
    ap.add_argument("--fixture", required=True)
    ap.add_argument("--gap-ms", type=float, default=15.0)
    ap.add_argument("--inject-mode", choices=["atomic", "chars"], default="atomic",
                    help="atomic = one write of the whole report, like a terminal; "
                         "chars = one byte at a time, kept for comparison")
    # Defaults to the session name encoded in the fixture filename, so any saved
    # session can be used, not only the one this harness was written against.
    ap.add_argument("--resume-arg", default="")
    ap.add_argument("--resume-menu", action="store_true",
                    help="launch bare --resume so the session picker renders, instead of resuming by name")
    ap.add_argument("--ready-marker", default="",
                    help="wait for this text instead of for quiescence (required for the picker)")
    ap.add_argument("--menu-pick", action="store_true",
                    help="after the picker settles, press Enter to open the highlighted session")
    ap.add_argument("--menu-prekeys", default="",
                    help="keys sent before Enter in the picker, e.g. 's' to reveal debug sessions")
    ap.add_argument("--cwd-dir", default="",
                    help="cwd for the client; defaults to a scratch directory under the run")
    ap.add_argument("--dismiss-overlay", action="store_true",
                    help="step past the first-run welcome overlay that a trivial session triggers")
    ap.add_argument("--rows", type=int, default=40)
    ap.add_argument("--cols", type=int, default=120)
    ap.add_argument("--ready-timeout", type=float, default=60.0)
    ap.add_argument("--quiet-s", type=float, default=0.9)
    ap.add_argument("--settle-s", type=float, default=2.5)
    ap.add_argument("--dump-only", type=float, default=0.0)
    ap.add_argument("--expect-bug", action="store_true",
                    help="negative control: the leak MUST reach the screen")
    args = ap.parse_args()

    run_dir = Path(args.run_dir).resolve()
    run_dir.mkdir(parents=True, exist_ok=True)
    fixture = Path(args.fixture).resolve()
    home, default_cwd = prepare_run(run_dir, fixture)
    cwd = Path(args.cwd_dir).resolve() if args.cwd_dir else default_cwd
    cwd.mkdir(parents=True, exist_ok=True)
    fixture_hash = sha256_file(home / "sessions" / fixture.name)
    (run_dir / "fixture.sha256").write_text(
        f"{fixture_hash}  {fixture.name}\n", encoding="utf-8")

    env = dict(os.environ)
    env.update({
        "JCODE_HOME": str(home),
        "KCODE_HOME": str(home),
        "TERM": "xterm-256color",
        "COLORTERM": "truecolor",
        "JCODE_NO_UPDATE": "1",
    })
    for leaky in ("JCODE_SESSION_ID", "JCODE_SERVER", "JCODE_SOCKET"):
        env.pop(leaky, None)

    argv = [args.binary, "--no-update", "--socket", str(home / "iso.sock"),
            "-C", str(cwd)]
    if args.resume_menu:
        argv.append("--resume")
    else:
        argv += ["--resume", args.resume_arg or resume_name(fixture)]

    result: dict = {
        "label": args.label,
        "binary": args.binary,
        "argv": argv[1:],
        "fixture": fixture.name,
        "fixture_sha256": fixture_hash,
        "gap_ms_target": args.gap_ms,
        "inject_mode": args.inject_mode,
        "checks": {},
        "notes": [],
    }

    proc = PtyProcess.spawn(argv, env=env, dimensions=(args.rows, args.cols))
    cap = Capture(args.rows, args.cols)
    threading.Thread(target=cap.pump, args=(proc,), daemon=True).start()

    screens = run_dir / "screen"
    screens.mkdir(exist_ok=True)

    def snapshot(name: str) -> str:
        text = cap.screen()
        (screens / f"{name}.txt").write_text(text, encoding="utf-8")
        return text

    try:
        if args.dump_only:
            time.sleep(args.dump_only)
            snap = snapshot("pilot")
            print(snap)
            print("=== alive:", proc.isalive())
            return 0

        settled = (wait_for_text(cap, args.ready_marker, args.ready_timeout)
                   if args.ready_marker
                   else cap.wait_quiescent(args.quiet_s, args.ready_timeout))
        result["ready_quiescent"] = settled
        result["ready_alive"] = bool(proc.isalive())
        before = snapshot("01-resumed")
        if not settled or not proc.isalive():
            result["checks"]["session_loaded"] = False
            result["notes"].append("terminal never settled with the session up")
        else:
            result["checks"]["session_loaded"] = True

        if args.dismiss_overlay and wait_for_text(cap, "Welcome to jcode", 10.0):
            # A session with almost nothing in it lands on the welcome screen, so
            # step past it the way a new user does: pick "New session" and confirm.
            proc.write("\x1b[B")
            time.sleep(0.4)
            proc.write("\r")
            time.sleep(2.5)
            proc.write("\x1b")
            time.sleep(0.8)
            result["overlay_dismissed"] = True
            before = snapshot("01a-after-overlay")

        if args.menu_pick:
            # Open the highlighted entry the way a user does. The default picker
            # view hides debug/canary sessions, so a recorded debug session needs
            # the filter toggled first.
            if args.menu_prekeys:
                for key in args.menu_prekeys:
                    proc.write(key)
                    time.sleep(0.4)
                time.sleep(1.2)
            proc.write("\r")
            picked = wait_for_text_gone(cap, args.ready_marker, args.ready_timeout)
            if picked:
                picked = cap.wait_quiescent(args.quiet_s, args.ready_timeout)
            result["menu_pick_settled"] = picked
            before = snapshot("01b-after-menu-pick")
            left_picker = "No session selected" not in before and "0 sessions" not in before
            has_composer = any(line.startswith("2>") for line in before.splitlines())
            result["checks"]["menu_resumed_session"] = bool(picked and left_picker and has_composer)
            result["menu_screen"] = before[:800]

        # Deliver the leaked report the way a terminal really emits one: a
        # single write of the whole CSI-u report. `chars` splits it per byte and
        # is kept only for comparison, because a lone ESC byte is not something
        # a terminal ever sends on its own.
        gaps: list[float] = []
        last = time.monotonic()
        if args.inject_mode == "atomic":
            if proc.isalive():
                proc.write(LEAK)
                gaps.append((time.monotonic() - last) * 1000.0)
            else:
                result["notes"].append("terminal was already gone before the report was sent")
        else:
            for char in LEAK:
                if not proc.isalive():
                    break
                proc.write(char)
                now = time.monotonic()
                gaps.append((now - last) * 1000.0)
                last = now
                time.sleep(args.gap_ms / 1000.0)
        result["gaps_ms"] = [round(g, 1) for g in gaps]
        result["gap_max_ms"] = round(max(gaps), 1) if gaps else None
        time.sleep(args.settle_s)
        after_leak = snapshot("02-after-leak")
        if not proc.isalive():
            result["notes"].append("terminal exited while injecting the report")

        result["checks"]["injection_within_burst_window"] = (
            bool(gaps) and max(gaps) < MAX_INTERCHAR_GAP_MS)

        # Positive control: ordinary typing must still reach the composer, so a
        # clean screen is evidence of stripping and not of a dead input path.
        for ch in POSITIVE:
            if not proc.isalive():
                break
            proc.write(ch)
            time.sleep(args.gap_ms / 1000.0)
        time.sleep(args.settle_s)
        after_positive = snapshot("03-after-positive")

        stream = cap.text()
        (run_dir / "pty-stream.txt").write_text(stream, encoding="utf-8")
        log_text = collect_log(home, run_dir / "app-log.txt")

        result["checks"]["leak_absent_after_injection"] = LEAK_TEXT not in after_leak
        result["checks"]["leak_absent_at_end"] = LEAK_TEXT not in after_positive
        result["checks"]["positive_control_present"] = POSITIVE in after_positive
        result["checks"]["log_skipped_kitty_reporting"] = (
            "kitty keyboard reporting is incompatible with the Windows console input decoder"
            in log_text)
        result["checks"]["scrub_logged_report_as_text"] = (
            f'Stripped a keyboard-protocol report that arrived as text: "[{LEAK_TEXT}u"' in log_text)
        result["checks"]["stream_emits_stack_neutral_clear"] = "\x1b[=0u" in stream
        result["checks"]["stream_decoded_losslessly"] = "\ufffd" not in stream

        if args.menu_pick:
            # The picker hands off to a child client. Under an isolated socket that
            # child does not outlive the handover here, so the composer cannot be
            # probed; the load itself is still checkable from the server log.
            result["checks"]["menu_loaded_session"] = (
                f"handle_get_history: session={fixture.stem}" in log_text)

        result["screen_head"] = after_positive[:800]
        result["pty_closed_early"] = not proc.isalive()
    finally:
        cap.alive = False
        try:
            proc.terminate(force=True)
        except Exception:
            pass

    checks = result["checks"]
    if args.menu_pick:
        # The picker hands the chosen session to a child client. In an isolated
        # ConPTY that child does not outlive the handover, so the keyboard part of
        # the scenario cannot be observed on this path. Say so instead of dressing
        # a partial observation up as a pass or a failure.
        result["verdict"] = (
            "MENU_SELECTED" if checks.get("menu_loaded_session") else "MENU_NOT_OBSERVED")
        result["notes"].append(
            "menu path stops at the handover; the composer is not reachable here")
        ok = result["verdict"] == "MENU_SELECTED"
    elif args.expect_bug:
        # Negative control: the harness must be able to SEE the bug. Green here
        # means "the leak was reproduced", which is what makes the fixed run's
        # clean screen meaningful.
        ok = (not checks.get("leak_absent_after_injection", True)) and checks.get(
            "positive_control_present", False)
        result["verdict"] = "BUG_REPRODUCED" if ok else "HARNESS_BLIND"
    else:
        ok = all(checks.values())
        if ok:
            result["verdict"] = "GREEN"
        elif (
            checks.get("leak_absent_after_injection")
            and checks.get("leak_absent_at_end")
            and checks.get("positive_control_present")
            and not checks.get("scrub_logged_report_as_text")
        ):
            # The screen is clean and typing works, but the app never logged a strip,
            # which is the only direct evidence the report arrived. That is a run that
            # proved nothing rather than a failed one, and calling it RED sends people
            # hunting for a regression that did not happen.
            result["verdict"] = "INCONCLUSIVE"
            result["notes"].append(
                "no strip logged: the report most likely never reached the composer; "
                "rerun, or use run_matrix.py where one run's delivery does not decide the result"
            )
            ok = False
        else:
            result["verdict"] = "RED"

    (run_dir / "result.json").write_text(json.dumps(result, indent=2), encoding="utf-8")
    print(json.dumps({k: result[k] for k in ("label", "verdict", "checks", "gap_max_ms")}, indent=2))
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
