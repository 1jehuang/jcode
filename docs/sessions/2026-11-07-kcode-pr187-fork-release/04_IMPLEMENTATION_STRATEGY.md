# Implementation Strategy — approach, rationale, alternatives

## Strategy 1: additive fork gate, not a fork branch of the workflow

`release.yml` is shared between upstream and fork. Rather than forking the workflow file (which would
diverge and rot), the gate condition admits **either** repository:

```yaml
github.repository == '1jehuang/jcode'
|| (github.repository == 'KooshaPari/KCode' && contains(github.ref_name, '-k'))
```

**Why the `-k` infix:** fork versions follow `BASEVERSION-<k|p>semver`, so every legitimate fork tag
carries `-k`. The infix is a cheap discriminator that cannot collide with upstream tags.

**Why a pre-build validator:** `fork_version::parse` runs **before** the build. A malformed fork tag
otherwise wastes a full build before failing. Failing early is both cheaper and clearer.

**Why Homebrew stays upstream-only:** the tap is an upstream-owned distribution channel. Publishing
fork artifacts into it would leak the fork onto upstream users.

## Strategy 2: gate at every publish surface AND the consumers

The original change gated 1 of 4 surfaces. Gating only the publisher is a **half-fix**: any consumer
that still expects the asset will fail. The correct shape is:

- gate all 4 publish paths (upload, blanket find, expected-asset validation, SHA256SUMS)
- fix the 2 SDK consumers that demanded ARM64 unconditionally

**Invariant:** publisher and consumer must agree on the asset set. If the publisher may omit an
asset, no consumer may require it.

## Strategy 3: budget the compile, not the test

The 600s timeout was not a slow-test finding. `jcode-embedding` compiles cold every run because the
workspace has **no `default-members`**. The measured test is 26s; the rest is compile.

**Choice:** raise to 1800s rather than add `default-members`. Adding `default-members` would change
the build graph for every job and risk pulling unrelated crates into scope. The timeout raise is a
one-line, low-blast-radius fix that targets the actual cost. If cold-compile cost becomes a broader
problem, revisiting `default-members` is the structural follow-up.

## Strategy 4: fix the flake at the scope, not at the lock

The gpt-5.6-sol flake looked like a lock-ordering race. It is not a lock bug: both tests hold
`lock_test_env()` and serialise correctly. The bug is that **one test wrote to a scope the other
test reads**, via a shared synthetic `"test"` credential.

Two candidate fixes were considered:

1. **Change the token** in one test so the scopes differ (avoids the collision).
2. **Populate the scope the credential actually reads** via
   `populate_account_models_for_scope(&provider.catalog_scope(), ...)`.

**Chosen: (2).** It matches the sibling test's existing pattern, so the test stops depending on the
accidental emptiness of a shared scope, and it is robust to future test additions. Option (1) treats
the symptom; option (2) removes the hidden coupling.

**Honest caveat:** the failing **order** was never reproduced deterministically locally. The
mechanism is established from code and the scope fingerprint matches exactly
(`api-key::9f86d081884c7d65`, i.e. `sha256("test")[0:16]`), but the acceptance evidence is 25/25
full-suite runs plus CI Build & Test step 17 (43m08s), not a local red/green reproduction.

## Strategy 5: untrusted interpolation removal

**Rule:** values arriving from `inputs` or `github` context are attacker-influenced. They must never
be expanded into shell source. Pass through `env:` and dereference as a shell variable.

Applied mechanically:

- 8 × `inputs.jcode_release_tag` -> `env: RELEASE_TAG`
- 3 × `github.actor` -> `env: GHCR_USER`

**Adjacent fix:** a redundant `env GHCR_TOKEN=... crane auth login` exposed the token in
`/proc/<pid>/cmdline`. SonarCloud flagged it, and editing the line made the **pre-existing** defect
count against new code, so it had to be removed in the same change.

**Verification discipline:** a repo-wide scan for `inputs.*`, `github.event.*`, `github.head_ref`,
`github.actor`, `github.triggering_actor` inside `run:` blocks returned **0**. That scan is the
acceptance artifact, not the number of edits.

## Strategy 6: desk host hardening

**export-wsl.ps1** was the root cause of runner death: `wsl --terminate` + `wsl --shutdown` SIGKILL
the listener. Three changes:

1. **Refuse while busy** — abort if any `Runner.Worker` is active in the distro (`-Force` overrides).
2. **Atomic promotion** — export to `<OutTar>.partial`, promote over the previous backup only on
   success, so a failed export cannot destroy the last good backup.
3. **Restart runners** — restart discovered runner services after the export completes.

**Runner reinstall:** the installations were deleted (only `_diag` survived). Reinstalled from
v2.338.2 linux-x64 with `--replace`, and a `runsvc.sh` wrapper was restored because the systemd
units `ExecStart=<runner-dir>/runsvc.sh`.

**Design principle:** a backup script must not be able to kill the thing it runs next to. The guard
plus atomic promotion converts a destructive window into a safe, observable abort.

## Process lessons applied

| Lesson | Rule |
|---|---|
| Rebase after squash-merge | Never `git rebase` a branch whose history was squash-merged. It replays the whole squashed history and conflicts. Use `cherry-pick` onto clean master; force-push is blocked by policy so a fresh branch name is needed. |
| Editor deadlock | Never run a git command that can open an editor without `GIT_EDITOR=true`. |
| YAML validity vs presence | "YAML is valid" is NOT "the value exists". Every step dereferencing `$VAR` must define `VAR` in its own `env:`. Duplicate mapping keys are legal YAML and PyYAML keeps the LAST, silently dropping earlier keys. |
