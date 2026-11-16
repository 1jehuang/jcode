# Windows keyboard-protocol leak: reproduction and stability proof

2026-11-16. Scratch root: `C:\Users\koosh\Agents\scratch\kitty-repro\`.
Everything below is a command that was run or an artifact that exists; nothing is inferred.

## Verdict

| Evidence | Result |
| --- | --- |
| 10 identical ConPTY runs against the fixed build, zero retries | **10/10 GREEN**, every check 10/10 |
| The same matrix run from the committed files themselves | **10/10 GREEN**, `runs/matrix-shipped-10` |
| 2 negative controls (same probe, pre-scrub build) | **2/2 BUG_REPRODUCED** |
| 5 identical VHS-driven runs against the fixed build | **5/5 GREEN** |
| Fixture bytes across all 10 runs | 1 distinct SHA-256, unchanged and still read-only |
| Recorded A/B (GIF + MP4 + PNG) | fixed composer clean, control composer leaks |

The controls matter more than the greens: they show the probe can see the bug, so a clean
screen means "stripped", not "the harness never delivered anything".

## The two builds

| Role | Commit | SHA-256 of `jcode.exe` |
| --- | --- | --- |
| fixed | `5f70a539d` (`v0.99.1227-dev`) | `d88d73b78336cce6efff97382a92e88a1610a8990c525363239d15f9e2ac8c24` |
| control | `9a18da13a` (`v0.99.1224-dev`) | `33353cb919c12f8b1d7e6619614ccd17207219ba9d67fc13461e48148307107e` |

The control is the same branch three commits earlier. It already writes `CSI = 0 u`
(`c9b17e6fc`) but does not have the decoder-side scrub (`5f70a539d`), so the only difference
under test is the scrub itself. Pinned copies live in `bin\`, hashes in `bin\BINARIES.txt`.

## Frozen subject

`fixture/master/session_mosquito_1794671874260_10867a3abffb9231.json`
- copied from `~/.jcode/sessions/`, 338 479 bytes, marked read-only;
- SHA-256 `00eb6e6ce73e614c59d38e2c7ec121e889383ccbb64b0034255137eff1d2f7ef`;
- resumed **by name** (`--resume mosquito`), not by id, so the name path is what is exercised.
  `find_session_by_name_or_id` resolves it from the file stem, so one fixture file in an
  isolated `sessions/` resolves deterministically with no title search involved.

Each run recopies the fixture into a fresh `JCODE_HOME`, uses a private daemon socket
(`--socket <home>\iso.sock`) and a scratch `-C` cwd. The live daemon and the real session store
are never touched.

## What a run does

1. Spawn the binary in a real ConPTY, 40x120, isolated home, private socket.
2. Wait for the terminal to go quiet (no output for 0.9s, 60s cap) so the resumed transcript
   is up and the composer owns input.
3. Emit the leaked report **exactly as a kitty terminal emits one**: a single write of
   `ESC [ 1 1 9 ; 1 : 3 u` (`w`, no modifiers, key-release event). The probe plays a *stale*
   terminal that has progressive-enhancement flags already armed, which is the real herdr
   case: a previous process left reporting on and this decoder cannot consume CSI-u.
4. Wait, snapshot the VT screen.
5. Type `zzqx` as a positive control and snapshot again.

## The nine checks, and why each exists

| Check | What it rules out |
| --- | --- |
| `session_loaded` | a run that never got to a composer |
| `injection_within_burst_window` | a red caused by slow injection (scrub allows <750ms gaps, <2s total) |
| `leak_absent_after_injection` | the reported bug itself |
| `leak_absent_at_end` | the fragment reappearing after later keystrokes |
| `positive_control_present` | a clean screen from a dead input path |
| `scrub_logged_report_as_text` | "clean" because the report never arrived, instead of arriving and being stripped |
| `log_skipped_kitty_reporting` | the negotiation fix being silently absent |
| `stream_emits_stack_neutral_clear` | `CSI = 0 u` not being written at startup |
| `stream_decoded_losslessly` | asserting on a lossy decode (U+FFFD present) |

## Observed

Composer row, fixed build (`runs/matrix-shipped-10/fixed-01/screen/`):

```
02-after-leak:      2>
03-after-positive:  2> zzqx
```

Composer row, control build (`runs/matrix-shipped-10/control-pre/screen/`):

```
02-after-leak:      2> [119;1:3u
03-after-positive:  2> [119;1:3uzzqx
```

So on the pre-scrub build the report lands in the prompt and ordinary typing keeps appending
after it, which is the user-visible bug. On the fixed build the report is removed and typing
still works.

App log from the fixed build (`runs/matrix-shipped-10/fixed-01/app-log.txt`):

```
[2026-11-16 02:29:26.445] [INFO] Stripped a keyboard-protocol report that arrived as text: "[119;1:3u"
```

and the negotiation line:

```
[INFO] Keyboard enhancement skipped: kitty keyboard reporting is incompatible with the Windows console input decoder; cleared inherited reporting instead
```

## The one flake I hit, and what it actually was

A first 10-run matrix scored **9/10**. The failing run's only failing check was
`scrub_logged_report_as_text`, while the screen was clean and the positive control passed.
Its artifacts answered it without a re-run: `pty-stream.txt` never contained the fragment and
`app-log.txt` had no scrub line, so the report had not arrived as text at all. A later run
could have been passed off as green by "re-running until it works"; the log assertion is what
refused that.

Cause: that matrix injected the report **one byte at a time, 15ms apart**. A lone ESC written
on its own is not something a terminal ever sends; it gets consumed as an Escape key press, and
whether the rest arrives as text then depends on ConPTY coalescing. The faithful model is a
single write of the whole report, which is what a terminal does, and is what the
`atomic` mode now does. The 10/10 matrix is atomic mode. The chars-mode matrix is kept at
`runs/matrix-10/` as the record of that finding: it is a harness artifact, not product
flakiness, and it is the reason the harness now records the injection mode per run.

## VHS

VHS is used as a second, independent driver, not as the assertion engine.

- VHS 0.12.1 built from source (`go install`, `bin\vhs.exe`); it needs `ttyd`, and
  `bin\ttyd.exe` is 1.7.7 `ttyd.win32.exe` verified against the published `SHA256SUMS`
  (`e33a27501b10b96981335bcba938b1145c7f52551a343e72160f00ab71832b37`).
- A tape can carry the real report: VHS's lexer keeps quoted strings byte for byte
  (`lexer.readString` performs no unescaping) and `ExecuteType` presses each rune, so an ESC
  byte written literally inside the quotes becomes a key press. `harness/make_tapes.py`
  generates the tapes with exactly one ESC each, so they are reproducible rather than
  hand-edited.
- Artifacts: `tapes/ab-fixed.{gif,mp4,png}` (composer ends at `2> zzqx`) and
  `tapes/ab-control.{gif,mp4,png}` (composer ends at `2> [119;1:3uzzqx`).
- The 5-run VHS matrix asserts on the isolated home's log, because a tape that merely records
  proves nothing: `runs/vhs-matrix-5/vhs-summary.json` is 5/5 with `tape_recorded`,
  `scrub_logged_report_as_text`, `log_skipped_kitty_reporting` and `session_present` true in
  every run. The PNG is for human eyes, not a pass/fail criterion.

## Limits

- The probe deliberately plays a **stale/hostile terminal** that ignores the app's clear and
  sends CSI-u anyway. That is the real-world "host arms its own keyboard mode" case the code
  comment describes, but it is not a recording of herdr from the Mac. Same Windows input
  stack, different host integration.
- Not exercised: the `--resume` **menu** (no argument). Only the by-name path
  (`--resume mosquito`) and, in earlier work, by-id.
- One host, one session fixture, one terminal size (40x120), one gap setting.
- The scrub is burst-scoped: a fragment typed with gaps over 750ms, or over 2s in total, is
  deliberately left alone as ordinary text. The harness records the observed gap per run
  (`gap_max_ms`, 0.0-16.0ms here) rather than assuming it.
- Other Windows hosts (VS Code terminal, conhost without ConPTY) and the release channel are
  not covered by this matrix.
- The evaluation is of the installed binary and pinned copies, not of a fresh `cargo build`
  of the branch head.

## CI on the commit that carries this harness

- `Build & Test (windows-latest)`: **pass**, 22m56s. The suite that covers the fixed code path
  is green on the platform the leak belongs to.
- `Build & Test (ubuntu-latest)`: 3683 passed, 3 failed. All three are pre-existing on this
  branch and were already failing on PR #2184:
  `tui::app::tests::test_cancel_image_only_failover_restores_attachments`
  (`crates/jcode-tui/src/tui/app/tests/remote_failover_routes.rs:983`),
  `test_compacted_history_event_applies_expanded_window`
  (`.../remote_events_reload_03/part_02.rs:373`), and
  `test_definitive_exhaustion_immediate_switch_and_manual_safety`
  (`.../support_failover/part_01.rs:682`). None of those files is touched by this branch.
- `Security preflight (Linux)`: 3 rustsec advisories in dependencies. The seven commits on this
  branch change no `Cargo.toml` or `Cargo.lock`; the diff is confined to `crates/jcode-tui`,
  `src/cli` and `scripts/repro`, so the advisories are inherited.
- `Format` and `Quality Guardrails`: red on the base commit and on every open PR, not from this
  branch. `scripts/check_code_size_budget.py` locally reports only pre-existing Rust files.

## Reproduce

```bat
cd /d C:\Users\koosh\Agents\scratch\kitty-repro

rem 10 identical runs + 2 controls
python harness\run_matrix.py --out runs\matrix-shipped-10 --runs 10

rem per-check evidence counts
python harness\check_counts.py runs\matrix-shipped-10

rem recorded A/B
python harness\make_tapes.py
set "PATH=C:\Users\koosh\Agents\scratch\kitty-repro\bin;%PATH%"
bin\vhs.exe tapes\ab-fixed.tape
bin\vhs.exe tapes\ab-control.tape

rem 5 VHS-driven runs with log assertions
python harness\vhs_matrix.py --out runs\vhs-matrix-5 --runs 5
```

`harness/pty_probe.py --dump-only 9 ...` prints the settled screen of one run, which is how the
readiness logic was kept honest instead of guessed.
