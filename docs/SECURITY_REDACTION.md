# Security Redaction Guide

This guide documents the local secret-redaction rules that should stay true when
changing logs, persisted messages, desktop diagnostics, live verification output,
or stdin/password flows.

## Principles

- Never log complete API keys, OAuth tokens, passwords, Authorization headers, or
  provider credentials.
- Preserve protocol payloads only where the value is required for functionality,
  for example `stdin_response.input` while sending a password to the server.
- Redact before truncating so a short log limit cannot accidentally preserve the
  sensitive prefix.
- Prefer the existing redaction helpers before adding a new ad hoc pattern list.
- Add a focused test for every new sensitive surface.

## Current Redaction Surfaces

| Surface | File | Behavior |
| --- | --- | --- |
| Persisted/session message text | `src/message.rs` | `redact_secrets` removes common direct tokens, env-style API key assignments, and sensitive headers. |
| Structured auth/log events | `src/logging.rs` | Secret-like field names are replaced with `<redacted>`. |
| Tool-call logs | `src/logging.rs` | Tool input/output is passed through `redact_secrets` before truncation. |
| Desktop logs/stderr mirror | `crates/jcode-desktop/src/desktop_log.rs` | Common token shapes, sensitive headers, env assignments, and JSON secret fields are redacted before truncation. |
| Live verification ledger | `src/live_tests.rs` | Secret auth material is fingerprinted and evidence strings are sanitized before writing JSONL/coverage. |
| Environment snapshots | `src/agent/environment.rs` | Snapshot records metadata only; it must not include raw environment variables. |
| Desktop stdin/password UI | `crates/jcode-desktop/src/single_session.rs` | Password input is masked on screen. The raw response is sent only through the required protocol path. |

## Tests To Run

Use these focused checks after touching redaction, auth, logs, or stdin/password
flows:

```powershell
$env:HOME=$env:USERPROFILE
cargo test -p jcode redact_secrets --lib
cargo test -p jcode logging --lib
cargo test -p jcode live_verification_ledger_writes_events_and_coverage_without_secret --lib
cargo test -p jcode message --lib
cargo test -p jcode telemetry --lib
cargo test -p jcode-desktop desktop_log
cargo test -p jcode-desktop stdin_response
cargo test -p jcode-protocol stdin --all-targets
cargo check --workspace --all-targets
```

For broader auth changes, also run:

```powershell
cargo test -p jcode auth --lib
cargo test -p jcode cli::login --lib
cargo test -p jcode cli::auth_test --lib
cargo test -p jcode-protocol --all-targets
```

## Audit Searches

Start with these searches before changing a sensitive path:

```powershell
rg -n "password|is_password|api[_-]?key|secret|token|credential|Authorization|Bearer" src crates
rg -n "logging::|desktop_log::|println!|eprintln!|debug!|trace!|warn!|error!" src crates
rg -n "serde_json::to_string|write_json_line|stdin_response|stdin_request" src crates
```

## What Not To Do

- Do not redact `stdin_response.input` before it reaches the server. That would
  break interactive password prompts.
- Do not log full serialized protocol requests for debugging.
- Do not add raw environment snapshots.
- Do not add a new log sink without a redaction step or a test showing secrets
  are removed.

## Current Open Items

- Review TUI auth prompts and account-picker paths for accidental raw key echoes.
- Review provider error formatting for responses that may include request bodies.
- Review mobile protocol docs and UI treatment for `is_password`.
- Consider moving shared redaction into a small crate only if duplication grows
  beyond the current local helpers.
