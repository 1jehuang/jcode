# jcode-dev Binary Isolation Design

## Problem

When running `cargo build` in the jcode workspace, the server process reloads and interrupts other active chat sessions sharing the same socket. This makes it impossible to develop jcode while simultaneously using it for coding tasks.

## Root Cause

Both the production `jcode` binary and a development build share the same socket paths:
- API socket: `runtime_dir()/jcode-api.sock`
- Legacy socket: `runtime_dir()/jcode.sock`
- Runtime dir: `$JCODE_RUNTIME_DIR` > `$XDG_RUNTIME_DIR` > `$TMPDIR` > fallback

When `cargo build` produces a new binary and the server restarts, it reclaims the same socket, disrupting all connected clients.

## Solution: Environment-Based Isolation

The socket path resolution in `crates/jcode-harness-api/src/sockets.rs` already supports env var overrides:
- `JCODE_RUNTIME_DIR` — overrides runtime directory (socket parent)
- `JCODE_API_SOCKET` — overrides API socket path directly
- `JCODE_SOCKET` — overrides legacy socket path directly

**jcode-dev** uses these overrides to get a fully isolated runtime.

## Design

### 1. Separate Config Directory

| Path | Production | Development |
|------|-----------|-------------|
| Config | `~/.jcode/` | `~/.jcode-dev/` |
| Runtime | `$TMPDIR/jcode-$USER/` | `$TMPDIR/jcode-dev-$USER/` |
| API socket | `runtime/jcode-api.sock` | `runtime/jcode-api.sock` (isolated by runtime dir) |
| Legacy socket | `runtime/jcode.sock` | `runtime/jcode.sock` (isolated by runtime dir) |

### 2. Wrapper Script

A shell wrapper `jcode-dev` sets the environment and launches the dev binary:

```bash
#!/bin/sh
# jcode-dev — isolated development wrapper
# Uses separate config and runtime dirs so cargo build doesn't interrupt production sessions.

export JCODE_HOME="${JCODE_HOME:-$HOME/.jcode-dev}"
export JCODE_RUNTIME_DIR="${JCODE_RUNTIME_DIR:-$(mktemp -d)/jcode-dev-$(id -u)}"
export JCODE_CONFIG_DIR="${JCODE_CONFIG_DIR:-$HOME/.jcode-dev}"

exec "$(dirname "$0")/jcode-bin" "$@"
```

### 3. Binary Name

The workspace already has `autobins = false` and explicit `[[bin]]` entries. Add a `jcode-dev` binary entry:

```toml
[[bin]]
name = "jcode-dev"
path = "src/main.rs"
```

This reuses the same entrypoint. The env vars control isolation, not the binary code.

### 4. What Gets Isolated

| Resource | Isolated? | Mechanism |
|----------|----------|-----------|
| Socket files | YES | `JCODE_RUNTIME_DIR` |
| Config files | YES | `JCODE_HOME` / `JCODE_CONFIG_DIR` |
| Session history | YES | Config dir contains sessions |
| Log files | YES | `JCODE_LOG_DIR` if set |
| HERDR state | YES | HERDR reads from `JCODE_HOME` |
| Memory files | YES | Config dir contains memories |
| Binary on disk | NO | Same `target/debug/jcode-dev` |
| Provider keys | NO | Same env vars (shared) |
| Upstream repo | NO | Same source tree |

### 5. Safety Properties

1. **No socket collision**: Different `JCODE_RUNTIME_DIR` = different socket files
2. **No config collision**: Different `JCODE_HOME` = different config/state
3. **Same provider keys**: Both instances share API keys (intentional)
4. **Independent sessions**: Each instance manages its own session history
5. **Clean shutdown**: Killing jcode-dev does not affect jcode production

### 6. Implementation Steps

1. Add `[[bin]] name = "jcode-dev"` to `Cargo.toml` (trivial, reuses main.rs)
2. Create `scripts/jcode-dev` wrapper script
3. Document in README
4. Optional: Add `jcode-dev` to `PATH` via install script

### 7. Verification

```bash
# Terminal 1: Start production jcode
jcode

# Terminal 2: Start dev jcode (isolated)
jcode-dev

# Terminal 3: Build jcode (does NOT interrupt Terminal 1 or 2)
cargo build --bin jcode-dev

# Verify sockets are separate
ls /tmp/jcode-*/jcode-api.sock       # production
ls /tmp/jcode-dev-*/jcode-api.sock   # development
```
