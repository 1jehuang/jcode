# Security Deep Audit (KCode fork, 2026-10-17)

**Repo:** `/Users/kooshapari/CodeProjects/Phenotype/repos/jcode` (Rust workspace, jcode v0.85.1-k1.1.1)
**Scope:** Read-only static analysis of `crates/` Rust source + `cargo audit`
**Auditor:** Jcode agent session

---

## Summary

- **Total findings:** 14
- **By severity:** CRITICAL: 0, HIGH: 2, MEDIUM: 7, LOW: 5

---

## Findings

| # | File:Line | Category | Description | Severity |
|---|-----------|----------|-------------|----------|
| 1 | `Cargo.lock` (rustls 0.23.45) | Dependency | RUSTSEC-2026-0346: `Debug` impls for client session values expose TLS secrets. Fix: upgrade to >=0.23.46. | HIGH |
| 2 | `Cargo.lock` (rustls 0.23.45) | Dependency | RUSTSEC-2026-0349: Unauthenticated ServerHello can downgrade later connections to classical key exchange. Fix: upgrade to >=0.23.46. | HIGH |
| 3 | `Cargo.lock` (lru 0.16.3) | Dependency | RUSTSEC-2026-0253: Potential use-after-free due to lack of panic safety in `LruCache::pop()`. | MEDIUM |
| 4 | `Cargo.lock` (anyhow 1.0.100) | Dependency | RUSTSEC-2026-0190: Unsoundness in `Error::downcast_mut()`. | MEDIUM |
| 5 | `Cargo.lock` (event-listener 5.4.1) | Dependency | RUSTSEC-2026-0221: `!Send` tags can cross thread boundaries via `StackSlot`. | MEDIUM |
| 6 | `Cargo.lock` (rand 0.8.5) | Dependency | RUSTSEC-2026-0097: Rand is unsound with a custom logger using `rand::rng()`. | MEDIUM |
| 7 | `Cargo.lock` (rustls 0.23.45) | Dependency | RUSTSEC-2026-0348: Brotli certificate decompression can allocate up to 1 GiB before authentication. | MEDIUM |
| 8 | `Cargo.lock` (rustls 0.23.45) | Dependency | RUSTSEC-2026-0347: Buffered application data sent to outer server after ECH rejection. | LOW |
| 9 | `crates/jcode-tool-core/src/lib.rs:193-201` | Path Traversal | `resolve_path()` does not validate against `../` traversal. Used by read/write/edit/apply_patch tools. Mitigated by: AI model is the sole caller (trusted), and bash tool can already access arbitrary paths. | MEDIUM |
| 10 | `crates/jcode-app-core/src/tool/write.rs:69-73` | Path Traversal | `create_dir_all(parent)` will create arbitrary directory trees if the AI model supplies a traversal path. Same mitigation as #9. | MEDIUM |
| 11 | `crates/jcode-provider-openrouter-runtime/src/lib.rs:283-335` | Auth Bypass | `DirectNoAuth` mode allows unauthenticated requests to local endpoints (Ollama, LM Studio). Controlled by `JCODE_OPENROUTER_ALLOW_NO_AUTH` env var. By design for local inference. | LOW |
| 12 | `crates/jcode-setup-hints/src/windows_setup.rs:399-576` | Unsafe | ~25 `unsafe` blocks for Windows API calls (keyboard hooks, hotkeys, mutexes). Required for Windows setup hints. Properly scoped. | LOW |
| 13 | `crates/jcode-storage/src/active_pids.rs:134,197` | Unsafe | `libc::unlinkat` and `libc::kill` via unsafe. Required for POSIX process/file management. Properly scoped. | LOW |
| 14 | `crates/jcode-core/src/fs.rs:56-131` | Unsafe | Windows ACL/security descriptor manipulation via unsafe. Required for file permission hardening. Properly scoped. | LOW |

---

## Top-5 Highest-Risk Fixes

1. **Upgrade rustls from 0.23.45 to >=0.23.46** — Resolves 4 vulnerabilities including TLS secret exposure (RUSTSEC-2026-0346) and connection downgrade (RUSTSEC-2026-0349). This is the single highest-impact fix. Affects every provider runtime that uses TLS (all of them).

2. **Upgrade lru from 0.16.3** — RUSTSEC-2026-0253 is a use-after-free in `LruCache::pop()`. If the cache is used in any hot path with panic-inducing data, this could cause memory corruption. Check if a patched version exists.

3. **Upgrade anyhow from 1.0.100** — RUSTSEC-2026-0190 is an unsoundness in `Error::downcast_mut()`. Low practical risk since `downcast_mut` is rarely used, but should be patched.

4. **Add path traversal validation to `resolve_path()`** — While the AI model is the trusted caller, a defense-in-depth check (reject or canonicalize paths containing `../`) would harden against prompt injection attacks where a compromised model tries to read `/etc/shadow` or overwrite system files.

5. **Pin or audit event-listener and rand** — Both have soundness issues (RUSTSEC-2026-0221, RUSTSEC-2026-0097). Evaluate whether the specific code paths in jcode exercise the affected APIs.

---

## Category Detail

### 1. Hardcoded Secrets / API Keys
**Status: CLEAN**

No hardcoded secrets, API keys, or tokens found in production code. All credential access is via environment variables (`JCODE_OPENROUTER_API_KEY`, `ANTHROPIC_AUTH_TOKEN`, etc.) or OAuth token files. Test files contain example AWS keys and JWT tokens for testing secret-redaction logic — these are not real credentials.

The `jcode-storage` crate implements `harden_secret_file_permissions()` which restricts file permissions on credential files (mode 0600 on Unix, ACL-restricted on Windows). This is a good defensive practice.

### 2. Command Injection in Shell Calls
**Status: WELL-DEFENDED**

The bash tool (`crates/jcode-app-core/src/tool/bash.rs`) is the primary command execution surface. It includes:

- **Destructive command gate** (`bash_destructive_gate.rs`): Uses `jcode-command-risk` crate to assess commands at 4 risk levels:
  - `Safe` — No destructive potential. Runs immediately.
  - `Low` — Destructive but bounded (inside working dir, recoverable via git). Runs with logging.
  - `Confirm` — Destructive target cannot be determined statically. Requires model re-justification.
  - `Catastrophic` — Would destroy home, root, or credentials. Absolute deny, no override possible.

- **Shell quoting**: Proper single-quote escaping implemented (`value.replace('\'', "'\"'\"'")`).

- **Timeout enforcement**: Commands have configurable timeout (default, max 600s).

- **Risk**: The bash tool inherently executes arbitrary shell commands by design. The destructive gate mitigates catastrophic commands, but a prompt injection attack could still execute non-catastrophic commands (e.g., `curl` to exfiltrate data). This is an accepted risk for a coding agent.

### 3. Unsafe Blocks
**Status: PROPERLY SCOPED**

All `unsafe` blocks are in platform-specific code for legitimate low-level operations:

| Location | Purpose |
|----------|---------|
| `jcode-storage/src/active_pids.rs` | POSIX `unlinkat()` and `kill()` for process/file management |
| `jcode-storage/src/lib.rs` | `libc::geteuid()` for permission checks |
| `jcode-setup-hints/src/windows_setup.rs` | Windows API: keyboard hooks, hotkeys, mutexes, message loops |
| `jcode-core/src/stdin_detect.rs` | macOS proc info for stdin detection |
| `jcode-core/src/fs.rs` | Windows ACL/security descriptor manipulation |
| `jcode-core/src/console.rs` | Windows console mode |
| `jcode-core/src/env.rs` | Environment variable access |
| `jcode-harness-api-server/src/translate.rs` | UID check for file ownership |

No `unsafe` blocks were found in business logic, data parsing, or network code. All are in FFI/platform layers where `unsafe` is unavoidable.

### 4. Unwraps in Network/File Paths
**Status: LOW RISK**

Production code unwraps are limited to:
- `serde_json::to_string()` — Cannot fail for valid types (infallible serialization)
- `.parse::<usize>().unwrap_or(0)` — Safe fallback pattern
- `std::env::var(...).ok()` — Safe Option-based access

All other unwraps are in `#[test]` functions. Network and file IO paths use proper `?` error propagation or `.ok()` fallbacks.

### 5. Missing Input Validation on User-Facing APIs
**Status: ACCEPTABLE**

CLI/TUI input parsing uses safe patterns:
- `.parse::<T>().ok()` for numeric parsing
- `.unwrap_or(default)` for fallback values
- `trim().parse()` for whitespace handling

No `parse::<T>().unwrap()` found in non-test production code.

### 6. Insecure HTTP Calls
**Status: CLEAN**

All external HTTP connections use HTTPS. The `http://` strings found in source are:
- **URL normalizers** that strip `http://`/`https://` prefixes for hostname extraction (cursor agent transport)
- **WebSocket upgrades** (`ws://` from `http://` for local proxy connections)
- **Localhost/loopback** connections for OAuth callbacks and local inference endpoints
- **SVG namespace** (`http://www.w3.org/2000/svg`) — not a network call

The `normalize_api_base()` function in `jcode-provider-metadata` explicitly rejects non-private `http://` URLs (test: `http://8.8.8.8/v1` returns `None`).

### 7. Weak Crypto / Hardcoded IVs
**Status: CLEAN**

No use of MD5, SHA-1, DES, RC4, or hardcoded initialization vectors found. The `chacha20` crate appears in the dependency tree but was not flagged by `cargo audit` for cryptographic weaknesses. SHA-256 is used for checksum verification in the update system.

### 8. Path Traversal in File Operations
**Status: ACCEPTABLE RISK (with recommendation)**

The `resolve_path()` function in `jcode-tool-core` resolves relative paths against the working directory but does not reject `../` traversal sequences. This affects all file tools (read, write, edit, apply_patch, ls, agentgrep).

**Mitigating factors:**
- The AI model is the sole caller of these tools — there is no direct user input flowing into file paths
- The bash tool can already access arbitrary paths via shell commands
- The destructive command gate prevents catastrophic file operations

**Recommendation:** Add a defense-in-depth canonicalization check that at minimum logs when paths escape the working directory. This hardens against prompt injection scenarios where a compromised model attempts to read sensitive files.

### 9. Auth Bypass Risks
**Status: CLEAN**

No skippable authentication middleware found. The `DirectNoAuth` mode in OpenRouter runtime is an intentional design for local inference endpoints (Ollama, LM Studio) and is controlled by an explicit environment variable. Authentication for remote providers uses:
- OAuth tokens (Antigravity, Copilot, Gemini)
- API key environment variables
- Bearer token authentication
- File permission hardening on credential storage

### 10. Dependency Vulnerabilities
**Status: 4 VULNERABILITIES, 10 WARNINGS**

| Crate | Version | Advisory | Severity | Fix |
|-------|---------|----------|----------|-----|
| rustls | 0.23.45 | RUSTSEC-2026-0346 | 4.0 (medium) | >=0.23.46 |
| rustls | 0.23.45 | RUSTSEC-2026-0347 | 2.3 (low) | >=0.23.46 |
| rustls | 0.23.45 | RUSTSEC-2026-0348 | 3.7 (low) | >=0.23.46 |
| rustls | 0.23.45 | RUSTSEC-2026-0349 | 3.7 (low) | >=0.23.46 |

**Unmaintained crate warnings (not vulnerabilities):** bincode, paste, rustybuzz, ttf-parser, unsafe-libyaml.

**Soundness advisories:** anyhow (downcast_mut), event-listener (Send boundary), lru (use-after-free), rand (unsound with custom logger).

---

## Forward Recommendation

Upgrade rustls to >=0.23.46 immediately (resolves 4 advisories in one dependency bump), then evaluate lru/anyhow/rand upgrades. Add defense-in-depth path traversal validation to `resolve_path()` as a hardening measure against prompt injection.