# [BUG-011] master contains lossy rebase merge (#33); only partially fixed forward

**ID:** BUG-011
**Category:** bug
**Severity:** high
**File:** git history — commit 901b33108 (merge #33) + 0737fb93f (partial fix)
**Status:** open

## Description
`master` history contains `901b33108 refactor(decollision): rename jcode to kcode (#33)`, a ~2,260-
commit squash rebase that was lossy. Known casualties included a dropped test body in
`crates/kcode-tui-style/src/harmony.rs` (E0741) and type/import breakage in
`crates/kcode-setup-hints/src/lib.rs`. The current tip `0737fb93f fix(tui-style): restore
print_calibration_scores test body lost in rebase` restores only the harmony test body; the other
breakages were not verified fixed in the same change. The earlier plan to revert via PR #36 was not
taken; the branch fixed forward instead.

## Evidence
```
git -C ~/wt-jcode-audit log --oneline -3
# 0737fb93f fix(tui-style): restore print_calibration_scores test body lost in rebase
# 901b33108 refactor(decollision): rename jcode to kcode (#33)
# 046ea2af5 chore(deps): enable Dependabot via minimal config
```
- Merge/revert history documented in AGENTS.md ("PR #33 merged-then-reverted", "Revert PR open").

## Suggested fix
Run `cargo check --workspace` (and `cargo test --workspace --no-run`) at the current tip to confirm
whether the other lossy-rebase breakages (kcode-base, kcode-swarm-core, kcode-setup-hints) are
resolved. Add a regression test or CI gate that compiles every workspace crate on the merge target
before a large rebase lands. Prefer `git revert` of the merge for a clean known-good baseline if
breakage persists.

## Effort
- M (1-2h)
