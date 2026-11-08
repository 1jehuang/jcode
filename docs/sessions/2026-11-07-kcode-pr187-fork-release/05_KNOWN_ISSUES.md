# Known Issues — open items, risks, technical debt

## OPEN — highest priority

### K1. WSL distro rebooted spontaneously at ~02:13:42 local, 2026-11-07

**Impact:** killed running CI jobs. Any job in flight is lost and must be re-run.

**What is ruled out:**

- Windows did **not** reboot (up since 2026-11-05).
- No sleep/resume events were recorded.
- WSL idle timeouts are not the cause: `C:\Users\koosh\.wslconfig` has
  `vmIdleTimeout=2147483647` and `instanceIdleTimeout=-1`.

**What was observed but not established as causal:**

- Scheduled tasks `InstinctWSLFleet`, `lane-omni*`, and `omni-econ-sync` reference `/root/*.sh`
  files that **no longer exist**.
- `ResolutionHost` ran at 02:10:11, but it is a COM-handler task with no visible WSL action.

**Status: UNRESOLVED.** The trigger was NOT positively identified. Do not record a cause until one is
proven. Next step is diagnostic only: enumerate WSL lifecycle events around 02:13, capture
`%SystemRoot%\System32\winevt\Logs` for Hyper-V/WSL providers, and check for any process issuing
`wsl --shutdown` or a distro termination at that timestamp.

## RESOLVED (with residual notes)

### K2. export-wsl.ps1 killed runners

`WSLExport` task ran `C:\Users\koosh\export-wsl.ps1`, which begins with
`wsl --terminate FedoraLinux-44` then `wsl --shutdown`. Those SIGKILL the runner listener — journal
showed `Sending SIGKILL to runner listener` — and the job then hangs until GitHub times it out and
reports failure **with no steps executed**. While exporting, the distro reports state `Exporting` and
refuses `CreateInstance` with `Wsl/Service/CreateInstance/E_ILLEGAL_STATE_CHANGE`
("An install, uninstall, or conversion is in progress"), so runners cannot start at all for the
export's duration.

**Resolved:** script rewritten (worker guard, `.partial` promotion, runner restart). Dry-run verified
`ABORT: 5 GitHub Actions runner worker(s) active` with exit 2. Original preserved as
`export-wsl.ps1.bak-20261107`.

**Residual risk:** the scheduled task still exists. It is now safe when the guard works, but
`-Force` bypasses the guard, and a future editor could reintroduce a shutdown.

### K3. Runner installations deleted

`/home/kcode-run/actions-runner` and `/home/kcode-run2/actions-runner` were deleted (only `_diag`
survived), leaving both services in an ENOENT re-launch loop.

**Resolved:** reinstalled from v2.338.2 linux-x64, reconfigured with `--replace`, `runsvc.sh` wrapper
restored (systemd `ExecStart=<runner-dir>/runsvc.sh`). Both online with
`self-hosted,Linux,X64,desk-linux`.

**Residual risk:** the root cause of the deletion was not identified. If it recurs, reinstallation is
the recovery, not the fix.

### K4. gpt-5.6-sol flake

**Resolved** in `aa4e7183a` by populating the credential's actual scope. **Residual:** no
deterministic local reproduction of the failing order exists, so a future test that writes a
restricted catalog to a **shared synthetic scope** could reintroduce an equivalent coupling.
Recommendation: synthetic test credentials should use distinct tokens per test, or all catalog writes
should go through the scope the credential reads.

### K5. YAML duplicate mapping key

A duplicate `env:` mapping key was legal YAML; PyYAML keeps the **LAST**, so `RELEASE_TAG` was
silently dropped and `$RELEASE_TAG` would have expanded empty in a publish step. A second bug: a step
used `$RELEASE_TAG` without defining it in `env:`.

**Resolved** in PR #256 verification. **Residual:** neither defect is caught by YAML-validity checks
or `actionlint` alone; the guard is the repo-wide scan plus per-step env review.

## Technical debt / follow-ups

| ID | Item | Note |
|---|---|---|
| D1 | No `default-members` in workspace | Causes cold compiles for targeted crates. Timeout raise is a mitigation, not a fix. |
| D2 | Embedding cohort budget 1800s | Generous because it covers a cold compile. Should shrink if D1 is addressed. |
| D3 | Test-size ratchet now EXACT | Zero slack by design. Any legitimate growth requires an explicit budget edit. |
| D4 | Fork release gate keyed on `-k` infix | Correct for the current version scheme; revisiting is required if the scheme changes. |
| D5 | Scheduled WSLExport task retains `-Force` bypass | Operational convenience that re-opens the destructive path if misused. |

## Non-issues (recorded to prevent re-investigation)

- `test_openai_supports_codex_models` was **not** a lock-ordering bug. Both tests held
  `lock_test_env()` and serialised; order was merely the *trigger*, not the defect.
- The embedding test was **not** slow. It is 26s once compiled.
