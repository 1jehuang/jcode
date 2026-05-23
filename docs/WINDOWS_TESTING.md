# Windows Testing Guide

This guide captures the current Windows validation workflow for jcode. It is
intended for incremental stabilization work: run focused groups, fix exact
failures, and avoid hiding hangs behind one broad test command.

## Scope

Use this guide when touching:

- Windows transport / named pipes
- server reload and socket lifecycle
- remote TUI startup, reconnect, history, and queued dispatch
- auth/provider tests that read config or environment
- session import/picker paths
- CLI reload/self-dev/startup paths

## Baseline Setup

Run commands from the repository root:

```powershell
$env:HOME=$env:USERPROFILE
git status --short --branch
cargo check --workspace --all-targets
```

Expected branch for the current Windows stabilization line:

```text
codex/fix-desktop-windows-build
```

Expected fork remote:

```text
https://github.com/perceojon-creator/jcode.git
```

Do not push local stabilization commits to `https://github.com/1jehuang/jcode`.

## Why Tests Are Segmented

On Windows, a full broad run such as:

```powershell
cargo test -p jcode --lib
```

can take long enough to obscure which test is stuck. Prefer focused filters and
record the exact group that fails. Some filters are also broader than they look:
for example, `cli` matches names containing `client`, so use `cli::...` module
filters instead.

## Core Matrix

Run these first after broad Windows changes:

```powershell
$env:HOME=$env:USERPROFILE
cargo check --workspace --all-targets
cargo test -p jcode auth --lib
cargo test -p jcode provider --lib
cargo test -p jcode socket --lib
cargo test -p jcode server::socket --lib
cargo test -p jcode server::reload_state --lib
cargo test -p jcode server::client_session --lib
cargo test -p jcode import --lib
cargo test -p jcode session_picker --lib
```

## CLI Matrix

Use module filters, not the broad `cli` filter:

```powershell
$env:HOME=$env:USERPROFILE
cargo test -p jcode cli::args --lib
cargo test -p jcode cli::provider_init --lib
cargo test -p jcode cli::commands --lib
cargo test -p jcode cli::dispatch --lib
cargo test -p jcode cli::login --lib
cargo test -p jcode cli::selfdev --lib
cargo test -p jcode cli::startup --lib
cargo test -p jcode cli::terminal --lib
```

## Remote TUI Matrix

Use this when touching `src/tui/app/remote*`, `src/tui/backend.rs`, remote
startup/reload/history, or queued dispatch:

```powershell
$env:HOME=$env:USERPROFILE
cargo test -p jcode test_remote --lib
cargo test -p jcode test_handle_server_event_history --lib
cargo test -p jcode test_save_and_restore_reload_state --lib
cargo test -p jcode test_model_picker --lib
cargo test -p jcode test_copy_selection --lib
cargo test -p jcode test_mouse --lib
cargo test -p jcode auto_poke --lib
cargo test -p jcode autojudge --lib
cargo test -p jcode judge --lib
cargo test -p jcode overnight --lib
```

## Focused Server/Client Matrix

Useful after socket, reload, history, and remote lifecycle changes:

```powershell
$env:HOME=$env:USERPROFILE
cargo test -p jcode server::client_state --lib
cargo test -p jcode server::client_lifecycle --lib
cargo test -p jcode server::client_disconnect_cleanup --lib
cargo test -p jcode server::client_comm --lib
cargo test -p jcode server::comm_control --lib
cargo test -p jcode server::comm_session --lib
cargo test -p jcode server::client_session::tests::resume_tests --lib
```

## Tool/TUI Support Matrix

Useful after search, ambient, or workspace-client changes:

```powershell
$env:HOME=$env:USERPROFILE
cargo test -p jcode tui::app::remote --lib
cargo test -p jcode tui::app::input --lib
cargo test -p jcode tui::workspace_client --lib
cargo test -p jcode tui::login_picker --lib
cargo test -p jcode tool::session_search --lib
cargo test -p jcode tool::conversation_search --lib
cargo test -p jcode tool::selfdev --lib
cargo test -p jcode tool::ambient --lib
cargo test -p jcode gateway --lib
```

## Other Crates

Use these for crate-level sanity after workspace changes:

```powershell
$env:HOME=$env:USERPROFILE
cargo test -p jcode-build-support
cargo test -p jcode-core
cargo test -p jcode-terminal-launch
cargo test -p jcode-mobile-sim
cargo test -p jcode-desktop
```

## Diagnosing Hangs

List candidate stuck processes:

```powershell
Get-Process | Where-Object { $_.ProcessName -match 'jcode-1c109e52ad525c4e|cargo|rustc' }
```

If a test run is clearly stuck and must be cleared:

```powershell
Get-Process | Where-Object { $_.ProcessName -match 'jcode-1c109e52ad525c4e|cargo|rustc' } | Stop-Process -Force -ErrorAction SilentlyContinue
```

Then rerun the smallest failing test with output:

```powershell
$env:HOME=$env:USERPROFILE
cargo test -p jcode exact_test_name --lib -- --nocapture
```

Search likely hang patterns:

```powershell
rg -n "read_to_end|read_to_string|NamedPipe|RemoteConnection::dummy|Runtime::new|block_on|spawn\(" src crates
```

Common fixes:

- Do not use `read_to_end` on duplex streams whose peer may stay open.
- Prefer `BufReader::read_line` or a protocol delimiter.
- Wrap async socket/named-pipe operations in a Tokio runtime.
- Keep broad connect/probe loops bounded with timeouts.

## Windows Named Pipe Pitfalls

Tests that create `RemoteConnection::dummy()` may need an entered Tokio runtime
on Windows because the dummy transport uses named pipes:

```rust
let rt = tokio::runtime::Runtime::new().unwrap();
let _guard = rt.enter();
let mut remote = crate::tui::backend::RemoteConnection::dummy();
```

Socket liveness checks must handle Windows named-pipe states such as
`ERROR_PIPE_BUSY` as a sign that a listener may exist.

## Path and JSON Pitfalls

Do not interpolate `Path::display()` directly into hand-written JSON. Windows
backslashes can produce invalid escapes. Prefer:

```rust
serde_json::to_string(&path)
```

or compare parsed paths as `PathBuf`.

## Auth and Provider Isolation

Provider/auth tests should not depend on the developer's real `JCODE_HOME`,
provider config, or poisoned environment variables. Prefer temp homes and env
locks for tests that read global state.

Typical validation:

```powershell
$env:HOME=$env:USERPROFILE
cargo test -p jcode auth --lib
cargo test -p jcode provider --lib
```

## Reporting Format

For each Windows stabilization change, report:

```text
Branch:
Commit:
Files changed:
Failure reproduced by:
Fix:
Validation:
Residual risk:
Next step:
```

Do not call the area closed unless the relevant focused matrix passes and
`cargo check --workspace --all-targets` remains green.
