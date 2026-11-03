# Firefox cold-start acceptance

## Regression and observed outcome (2026-10-17, Pacific time)

The original failure had two components: an unreaped Linux process named
`firefox` counted as a running browser, suppressing normal autolaunch; setup then
passed the extension XPI to a generic file opener rather than Firefox.
Commit `2bb60448f` corrects both decisions.

This was verified with the deployed Jcode CLI, the installed Firefox executable,
and the actual Browser Agent Bridge extension/native host, not a mocked browser.
The extension package was version 0.9.7 in both runs. Each run started with a
fresh private Firefox profile containing that extension, no running Firefox,
and a real unreaped child process named `firefox` in state `Z`.

| Observation | Before: `7452234c1-dirty-097c42e2b5a4` | After: `ce5a995a5-dirty-5b08f1660926`, containing the fix |
| --- | --- | --- |
| `jcode browser setup firefox` | Entered repair and claimed to open the extension installer | Launched Firefox normally and reported the bridge reconnected |
| Generic XPI handler | Invoked `xdg-open`, which selected a terminal file handler in the isolated environment | Not invoked |
| Extension installation prompt | Reported; connection wait timed out | Not reported; output said no setup action was needed |
| `jcode browser status firefox` | Bridge not responding; incorrectly claimed Firefox was running | Bridge responding, compatibility OK, built-in browser tool ready |
| Real bridge `listTabs` | Failed, no tabs returned | One real Firefox tab, `about:blank`, no XPI tab |
| Acceptance result | Failed | Passed |

The pre-fix failure is expected, not a test-infrastructure failure. Setup's exit
code alone was insufficient: the old CLI returned zero even though the bridge
was not ready. The acceptance runner checks the actual readiness and tab result.
The installed post-fix binary was picked up by a concurrent Jcode reload; the
browser implementation was also inspected in that executable before testing.

## Reproduce safely on Linux

Requirements: installed Firefox, an already-installed Jcode browser bridge and
extension package, Python 3, `ip`, and permission to create unprivileged user,
PID, mount, and network namespaces. The runner refuses to proceed without the
private PID and network namespaces. It must not be run against the user's live
profile.

From the repository root:

```sh
unshare --user --map-root-user --pid --fork --mount-proc --net \
  --kill-child=SIGTERM /usr/bin/python3 scripts/verify_browser_cold_start.py \
  --host-netns "$(readlink /proc/self/ns/net)" \
  --output-dir "$JCODE_SCRATCH_DIR/browser-cold-acceptance"
```

Set `--jcode /absolute/path/to/jcode` to compare an older installed executable.
Use `--bridge-dir` to select a different installed bridge package. Keep the
bridge package identical for a before/after comparison. The default executable
is the resolved shared-server build, not an unrelated binary on `PATH`.

The runner copies only the bridge binaries and extension package. It creates
its own empty HOME, Firefox profile, native-host registration and runtime
directory, and starts real Firefox headlessly. It does not copy credentials or
user browsing data. Its private network has loopback only, so bridge ports cannot
collide with the user's bridge and no release update is downloaded. Consequently
setup prints an expected update-lookup failure, then continues to exercise the
installed bridge. Content sandbox disabling and extension sideload preferences
apply only to this isolated disposable profile.

The runner writes `setup.stdout`, `setup.stderr`, `status.stdout`, `status.stderr`,
`tabs.stdout`, `tabs.stderr` and `result.json` under its reported output directory.
Exiting the PID namespace terminates all of its browser and native-host processes.
The user's existing Firefox PID remained alive and its bridge still responded
after the runs.

## Other validation and limits

- `justrust check -p jcode-base`: passed.
- `justrust test -p jcode-base --lib browser_tests -- --test-threads=1`: 35 passed.
  This includes live/zombie process detection, Firefox/ESR/Flatpak routing,
  argument boundaries, and launcher failure.
- `cargo fmt --package jcode-base -- --check`: passed.
- Clippy was attempted but blocked by unrelated existing diagnostics in telemetry,
  session/config code, and `usage/flight_tests.rs`. This is not a lint-clean claim.
- The acceptance result covers Linux cold startup and real bridge communication.
  It does not establish macOS/Windows GUI behavior or a fresh interactive XPI
  installation. Those launch branches share the existing platform launcher.
- No compile-time or browser-startup speedup is claimed by this functional test.
