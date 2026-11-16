# Windows keyboard-protocol leak: PTY reproduction harness

A real-terminal reproduction for the leak where a terminal's kitty keyboard report
(`ESC [ 1 1 9 ; 1 : 3 u`) reached the composer as text on Windows (#224).

`PROOF.md` in this directory is the record of the run that motivated this directory:
10 identical single-run probes, all green, bracketed by negative controls that reproduce
the bug, plus a recorded VHS A/B.

## Why the shape of the evidence matters

A green screen on its own proves nothing. It can mean the fix worked, or that the report
never arrived. So every run asserts on four independent channels:

1. the VT screen after the report (`pyte`, not a rolled-up byte log);
2. the same screen after an ordinary-typing positive control;
3. the app's own log, which says either that a report "arrived as text" and was stripped,
   or nothing at all;
4. the PTY stream (`CSI = 0 u` present, no U+FFFD, so assertions are on a lossless decode).

The matrix additionally runs the **pre-fix binary as a negative control** before and after
the fixed runs, and fails the whole run if a control ever stops reproducing the bug. That is
what stops a blind probe from reporting green.

## Requirements

- Windows with ConPTY (the leak is Windows-console-input-decoder specific);
- Python 3.11 with `pywinpty` and `pyte`;
- a **local** session fixture: a copy of any `~/.jcode/sessions/session_<name>_*.json`.
  No session transcript is committed here. Freeze one with

  ```bat
  copy %USERPROFILE%\.jcode\sessions\session_mosquito_<id>.json fixture\master\
  attrib +R fixture\master\session_mosquito_<id>.json
  ```

- two binaries: the build under test, and a pre-fix build for the control;
- optional, for the recorded A/B: `vhs` (with `ttyd`) and a browser for it to drive.

## Running

```bat
rem one probe, and the pilot mode that prints the settled screen
python pty_probe.py --run-dir runs\one --fixture fixture\master\session_mosquito_<id>.json
python pty_probe.py --run-dir runs\pilot --fixture fixture\master\session_mosquito_<id>.json --dump-only 9

rem N identical runs bracketed by controls; fails unless all N are green and both controls leak
python run_matrix.py --out runs\matrix --runs 10 ^
  --fixture fixture\master\session_mosquito_<id>.json ^
  --binary path\to\jcode-fixed.exe ^
  --control-binary path\to\jcode-pre-fix.exe

python check_counts.py runs\matrix

rem recorded A/B through VHS
set JCODE_REPRO_FIXED_BIN=path\to\jcode-fixed.exe
set JCODE_REPRO_CONTROL_BIN=path\to\jcode-pre-fix.exe
python make_tapes.py
vhs tapes\ab-fixed.tape
vhs tapes\ab-control.tape

rem N VHS-driven runs, asserting on the isolated home's log
python vhs_matrix.py --out runs\vhs-matrix --runs 5 --binary path\to\jcode-fixed.exe
```

`--fixture`, `--binary` and `--control-binary` are also read from `JCODE_REPRO_FIXTURE`,
`JCODE_REPRO_FIXED_BIN` and `JCODE_REPRO_CONTROL_BIN`.

## What each file is

| File | Role |
| --- | --- |
| `pty_probe.py` | one run: ConPTY, isolated `JCODE_HOME`, private socket, single-write report, snapshots, checks, artifacts |
| `run_matrix.py` | N identical runs plus bracketing negative controls; refuses to pass unless every run is green |
| `check_counts.py` | per-check pass counts over a matrix directory |
| `make_tapes.py` | generates the VHS tapes, embedding the real ESC byte so the tape can emit the report |
| `vhs_matrix.py` | second, independent driver: VHS runs with log-based assertions |
| `resume_world.cmd` | builds a disposable world (fixture copy, isolated home) and resumes it by name |
| `PROOF.md` | the measured result: 10/10, 2/2 controls, 5/5 VHS, and the limits |

## Artifacts per run

`pty-stream.txt`, `screen/*.txt`, `app-log.txt`, `fixture.sha256`, `result.json`. A red run is
diagnosable from these; nothing needs re-running to find out which channel disagreed.

## Honest scope

The probe plays a *stale kitty terminal* that already has progressive-enhancement flags armed
and sends CSI-u regardless of the app's `CSI = 0 u` clear. That models the host that arms its
own keyboard mode, not a recording of a specific external terminal. The `--resume` menu path,
other terminal hosts, and non-ConPTY consoles are not covered.
