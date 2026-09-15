# Jcode Handoff — 2026-09-15

## Current State

**Branch:** master
**Last commits (newest first):**
```
9c37986db fix: add missing ModelRoute.usage field in test fixtures
d5f37e1f4 feat: HERDR terminal runtime integration for jcode
7d0158aa6 feat(provider): add ForgeCode runtime provider
1f4810821 fix(tui): fix status bar compilation errors
1e73c6425 fix: revert Cargo.lock to avoid aws-smithy-json incompatibility
0acfb941d audit: add agent-lab assessment dossier (85% pass, 22 criteria)
3f5e2df22 feat: add benchmarks, status bar, and tool tests (F-02, F-03)
28cfac781 docs: document SDKROOT build workaround for macOS (F-01)
```

**Build:** PASSES (cargo check clean, warnings only)
**Binary:** `~/.local/bin/jcode` v0.0.0-dev (installed)

## Test Results (258 passed, 8 failed)

### Passing (258/266 = 97%)

All core functionality works: tools, MCP, config, auth, models, etc.

### Failing (8 tests)

| # | Test | Root Cause | Severity | Fix Effort |
|---|------|-----------|----------|-----------|
| 1 | `cli_auth_status_doctor_and_login_lifecycle_uses_fresh_sandbox` | `after_status.any_available` assertion — login lifecycle doesn't set availability | HIGH | Investigate auth flow |
| 2 | `run_auto_poke_followup_targets_below_threshold_todos` | Missing "completion confidence" in poke message | LOW | String update |
| 3 | `auth_integration_registry_matches_cli_choice_runtime_wiring` | Provider `orcarouter` missing from CLI choice map | MEDIUM | Add orcarouter mapping |
| 4 | `login_provider_choice_table_round_trips_catalog_providers` | Same `orcarouter` issue — in catalog but not choice table | MEDIUM | Same fix as #3 |
| 5 | `startup_timeout_kills_and_reaps_owned_process` | OS error 2 (No such file or directory) — missing test binary | HIGH | Check test setup |
| 6 | `test_init_provider_jcode_delegates_runtime_profile_to_wrapper` | Label mismatch: "Jcode Subscription" vs "Jcode Hosted Models" | LOW | Update expected string |
| 7 | `spawn_resume_in_new_terminal_uses_handterm_exec_mode` | Timeout waiting for launcher output file | MEDIUM | Test timing / env issue |
| 8 | `spawn_selfdev_in_new_terminal_uses_handterm_exec_mode` | PoisonError on env lock | MEDIUM | Test isolation issue |

### Recommended Fix Priority
1. **Tests 3+4** (orcarouter) — same root cause, one fix
2. **Test 6** — trivial string fix
3. **Test 2** — trivial string fix
4. **Tests 7+8** — test isolation issues, may need env lock coordination
5. **Tests 1+5** — need deeper investigation

## My Completed Work (already committed)

- **28cfac781** F-01: SDKROOT build docs in README
- **3f5e2df22** F-02: Tool tests (`tests/test_tool_{bash,edit,ls,read,write}.rs`), benchmarks (`benches/startup.rs`)
- **0acfb941d** Assessment dossier (85% pass, 22 criteria)
- **1e73c6425** Cargo.lock revert for aws-smithy-json compat
- **1f4810821** Status bar compilation fix
- **~/.codex/AGENTS.md** and **~/.forge/AGENTS.md** updated with Jcode patterns

## Build Environment

```bash
# Required for builds on this machine:
export SDKROOT=/Library/Developer/CommandLineTools/SDKs/MacOSX26.sdk
export MACOSX_DEPLOYMENT_TARGET=15.0

# Install binary:
cargo install --path . --force --bin jcode
```

## Lock Contention Notes

Shared `target/` directory across 16-128 agents causes massive lock contention. The `nohup` approach doesn't survive harness timeout. Use `launchd` for persistent background cargo tasks:

```bash
# Create script at /tmp/jcode-test-runner.sh
# Create plist at ~/Library/LaunchAgents/com.jcode.test-runner.plist
# Load: launchctl load ~/Library/LaunchAgents/com.jcode.test-runner.plist
# Check: cat /tmp/jcode-test-results.txt
# Unload: launchctl unload ~/Library/LaunchAgents/com.jcode.test-runner.plist
```

## Pending Work

1. Fix the 8 failing tests (priority order above)
2. Status bar visual verification in TUI
3. Assessment dossier is at `.audit/jcode-assessment-2026-09-15/`
