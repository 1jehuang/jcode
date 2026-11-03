# [SEC-003] Verify no secrets are logged (preventive; no leak found yet)

**ID:** SEC-003
**Category:** security
**Severity:** low
**File:** repository-wide (logging call sites)
**Status:** open

## Description
A targeted scan for `trace!/debug!/info!` calls touching token/secret/password/api-key/credential
identifiers returned no matches on the current tree, which is reassuring but not a guarantee: the
absence of `tracing`/`log` imports (`use tracing` and `use log::` both matched 0 files) means
logging may go through custom macros that this scan did not cover. Auth tokens are read from the
environment/keychain and could reach a debug formatter.

## Evidence
```
grep -rInE "trace!|debug!|info!" --include="*.rs" crates src | grep -iE "token|secret|password|api_?key|credential" | grep -v test
# -> no matches
grep -rIl "use tracing" --include="*.rs" crates src | wc -l   # -> 0
grep -rIl "use log::"   --include="*.rs" crates src | wc -l   # -> 0
```

## Suggested fix
Locate the actual logging macros used by the crate, then re-run the secret scan against them. Add a
redaction wrapper/type for tokens and a CI check (e.g. `gitleaks`) plus a unit test that a sample
config struct's `Debug` output does not contain a secret. Never log `Authorization` headers.

## Effort
- S (< 30m)
