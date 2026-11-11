# Windows MCP pool regression harness

These ignored tests create only dedicated fixture processes. Use the controller;
setting JCODE_MCP_TEST_CONTROLLER manually does not establish containment.

- `production_pool_overlap` checks held-reference scheduling, stale-session
  release, exact Job membership and identity, successor RPC, and eventual
  retained-handle completion before controller teardown.
- `production_pool_accounting_observations` runs the same scenario with buffered
  accounting, PID-list, and native spawn-return samples. It does not assert
  predecessor completion before successor creation or active execution overlap.

Run the controller selection and cleanup fault-injection checks first:

```powershell
python -m unittest discover -s crates/jcode-base/tests/support/windows_mcp -p 'test_*.py'
```

Build the two integration binaries on Windows with Python 3 available:

```powershell
$env:CARGO_BUILD_JOBS = '1'
$env:CARGO_PROFILE_TEST_DEBUG = '0'
cargo --config 'build.rustc-wrapper=""' test -p jcode-base --features test-support --test production_pool_overlap --test production_pool_accounting_observations --no-run
```

Select the exact emitted executable, copy it to an immutable evidence path and
record its SHA256, then run one exact ignored test per controller invocation:

```powershell
python crates/jcode-base/tests/support/windows_mcp/atomic_controller.py <regression.exe> overlapping_release_reconnect_preserves_successor_ownership --exact --ignored --nocapture --test-threads=1
python crates/jcode-base/tests/support/windows_mcp/atomic_controller.py <observations.exe> accounting_and_spawn_samples_do_not_prove_strict_completion --exact --ignored --nocapture --test-threads=1
```

The controller creates the test suspended with creation-time Job assignment and
an explicit inherited-handle list. It verifies the executable image and Job
membership before resuming, isolates configuration directories/environment, and
checks that an outside-Job negative control survives. Cleanup closes only its
owned fixture Job. Global process-name kills and live services are unnecessary.
The Python server and Win32 structure helpers are minimal extracts of the audited
fixture. The controller adds exact selected-test validation and preserves durable
stdout/stderr/receipt artifacts under the printed MCP_CONTROLLER_ARTIFACTS path.
Process exit zero alone cannot pass: exactly one selected test must pass, with
zero failures/ignored/measured tests; the outside control is a separate result.

Samples are sequential, not atomic. WAIT_TIMEOUT on a process handle does not
prove the process is executing. An empty Job PID list or ActiveProcesses=0 does
not establish all retained process handles have signaled. API failure makes
PID-list counts/IDs unknown; truncated IDs are partial. Buffered samples flush on
Rust panic; a hard process abort can prevent buffered Rust samples from being emitted and is
inconclusive. Controller timeout/error preserves raw stdout/stderr and a receipt
with controller_success=false; cleanup failure cannot become a success. Keep every failed run alongside later successful observations.

This harness changes no production lifecycle behavior and does not repair the
strict predecessor-completion gate. No process leak, resource collision, or
strict absence of generation overlap is inferred from a passing regression.
