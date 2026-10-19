# Task D: Startup Spawn Notice — Dead Code Finding (verified 2026-10-18T09:00Z)

## Verdict: genuinely dead code. Recommend DELETE unless the notice is wanted.

## Evidence

`startup_spawn_hint_dismissed` has exactly **4 references repo-wide**, all in 2 files:

| # | Location | Role |
|---|---|---|
| 1 | `crates/jcode-setup-hints/src/lib.rs:77` | struct field declaration |
| 2 | `crates/jcode-setup-hints/src/lib.rs:141` | default value = `true` |
| 3 | `crates/jcode-setup-hints/src/lib.rs:612` | the gate: `if !startup_spawn_hint_dismissed` |
| 4 | `scripts/update_rehearsal.py:213` | test fixture that **persists `True`** into `~/.kcode/setup_hints.json` |

The notice itself is emitted at `crates/jcode-setup-hints/src/lib.rs:616`.

## Why it is unreachable

- The field defaults to **`true`** (dismissed) at line 141.
- **No code path in the repo ever writes `false`.** The only writer found is
  `scripts/update_rehearsal.py:213`, which writes **`True`** — i.e. it writes the
  dismissed state, reinforcing the default rather than clearing it.
- With `startup_spawn_hint_dismissed == true` for every run, the gate at line 612
  `if !startup_spawn_hint_dismissed` is never taken, so line 616 never executes.
- Result: the notice text is emitted **zero times in production**.

## Secondary defect: stale branding

Even if it were reachable, the notice string still says **`"jcode"`** rather than
`"kcode"` — i.e. it would ship pre-rename branding in a post-rename binary.

## Recommendation

**Delete the field, the default, the gate, and the notice text** (lines ~77, ~141,
~612, ~616). Do not attempt to "fix" it by adding a writer, because there is no
intent signal anywhere that a user wants this notice — adding one would be
inventing a feature.

If the operator wants the notice as a real feature, that is a separate decision
requiring a writer path and a `kcode`-corrected string. Flagging, not doing.

## Related: Task A deadlock fix has landed separately

The resume-test deadlock (stranded commit `c89a0d6aa`) was re-landed as a clean
cherry-pick off current master in `/Users/kooshapari/.jcode/scratch/wt-resume-relend`
(`b99b19bc8`), verified: compiles clean, the 7 previously-hanging resume tests all
pass, full suite pending. See the landing session for that evidence.
