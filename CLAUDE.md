# jcode Development Guidelines

## Workflow

- **Commit as you go** - Make small, focused commits after completing each feature or fix
- **Push when done** - Push all commits to remote when finishing a task or session
- **No AI co-author** - Never include `Co-Authored-By` lines in commits
- **Rebuild and install when done** - Run `cargo build --release && scripts/install_release.sh`
- **Test before committing** - Run `cargo test` to verify changes
- **Bump version for releases** - Update version in `Cargo.toml` when making releases

## Versioning

jcode uses **auto-incrementing** semantic versioning (`v0.1.X`).

**Automatic (patch):**
- Build number auto-increments on every `cargo build`
- Stored in `~/.jcode/build_number`
- Example: `v0.1.1` → `v0.1.2` → `v0.1.3` ...

**Manual (major/minor):**
- For big changes, manually update major/minor version in `Cargo.toml`
- **Minor** (0.1.x → 0.2.0): New features, significant enhancements
- **Major** (0.x.x → 1.0.0): Breaking changes to CLI, config, or APIs

The build also includes git hash and `-dev` suffix for uncommitted changes (e.g., `v0.1.47-dev (abc1234)`).

## Testing Changes

This repo has self-dev mode. When running `jcode` in this directory:
- It auto-detects the jcode repo and enables self-dev mode
- Builds and tests a canary version before running
- Use `/reload` to hot-reload after making changes

**Manual testing** - After making changes, manually test the feature in a real terminal to verify it works. Use kitty to launch test instances:
```bash
sock=$(ls /tmp/kitty.sock* | head -1)
kitten @ --to unix:$sock launch --type=os-window ./target/release/jcode --standalone
```

## Commands

```bash
cargo build --release && scripts/install_release.sh  # Build and install (versioned + symlink)
cargo test              # Run all tests
cargo test --test e2e   # Run only e2e tests
```

## Logs

Logs are written to `~/.jcode/logs/` (daily files like `jcode-YYYY-MM-DD.log`).

## Install Notes

- `scripts/install_release.sh` installs a versioned binary and atomically flips the `~/.local/bin/jcode` symlink.
- Ensure `~/.local/bin` is **before** `~/.cargo/bin` in `PATH` so the symlinked release is used.

## Environment Setup

The Claude provider uses the Claude Code CLI (no Python SDK required). Optional overrides:

```bash
export JCODE_CLAUDE_CLI_PATH=~/.local/bin/claude
export JCODE_CLAUDE_CLI_MODEL=claude-opus-4-5-20251101
export JCODE_CLAUDE_CLI_PERMISSION_MODE=bypassPermissions
export JCODE_CLAUDE_CLI_PARTIAL=1
```

## Key Files

- `src/main.rs` - Entry point, CLI, self-dev mode
- `src/tui/app.rs` - TUI application state and logic
- `src/tui/ui.rs` - UI rendering
- `src/tool/` - Tool implementations
- `src/id.rs` - Session naming and IDs

## Headless Testing via Debug Socket

jcode has a debug socket for headless/automated testing. This allows external scripts to:
- Execute tools directly (bypass LLM)
- Send messages to the agent and get responses
- Query agent state and history
- Spawn and control test instances

### Enable Debug Control

```bash
# Option 1: File toggle (persists, no restart needed after reload)
touch ~/.jcode/debug_control

# Option 2: Environment variable
JCODE_DEBUG_CONTROL=1 jcode serve
```

### Socket Paths

- Main socket: `/run/user/$(id -u)/jcode.sock`
- Debug socket: `/run/user/$(id -u)/jcode-debug.sock`

### Debug Commands (Namespaced)

Commands can be namespaced with `server:`, `client:`, or `tester:` prefixes. Unnamespaced commands default to server.

**Server Commands** (agent/tools - default namespace):
| Command | Description |
|---------|-------------|
| `state` | Agent state (session, model, canary) |
| `history` | Conversation history as JSON |
| `tools` | List available tools |
| `last_response` | Last assistant response |
| `message:<text>` | Send message, get LLM response |
| `tool:<name> <json>` | Execute tool directly |
| `sessions` | List all sessions |
| `create_session` | Create headless session |
| `help` | List commands |

**Client Commands** (TUI/visual debug - `client:` prefix):
| Command | Description |
|---------|-------------|
| `client:frame` | Get latest visual debug frame (JSON) |
| `client:frame-normalized` | Get normalized frame (for diffs) |
| `client:screen` | Dump visual debug frames to file |
| `client:enable` | Enable visual debug capture |
| `client:disable` | Disable visual debug capture |
| `client:status` | Get client debug status |
| `client:help` | Client command help |

**Tester Commands** (spawned instances - `tester:` prefix):
| Command | Description |
|---------|-------------|
| `tester:spawn` | Spawn new tester instance |
| `tester:spawn {"cwd":"/path"}` | Spawn with options |
| `tester:list` | List active testers |
| `tester:<id>:frame` | Get frame from tester |
| `tester:<id>:state` | Get tester state |
| `tester:<id>:message:<text>` | Send message to tester |
| `tester:<id>:stop` | Stop tester |

### Python Test Example

```python
import socket
import json

def debug_cmd(cmd, session_id, timeout=30):
    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    sock.connect('/run/user/1000/jcode-debug.sock')
    sock.settimeout(timeout)
    req = {'type': 'debug_command', 'id': 1, 'command': cmd, 'session_id': session_id}
    sock.send((json.dumps(req) + '\n').encode())
    data = sock.recv(65536).decode()
    sock.close()
    return json.loads(data)

# Get session first by subscribing to main socket
main_sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
main_sock.connect('/run/user/1000/jcode.sock')
main_sock.settimeout(10.0)
req = json.dumps({'type': 'subscribe', 'id': 1,
                 'working_dir': '/home/jeremy/jcode', 'selfdev': True}) + '\n'
main_sock.send(req.encode())
# Parse response to get session_id...

# Server commands (default namespace)
result = debug_cmd('state', session_id)
result = debug_cmd('tool:bash {"command":"echo hello"}', session_id)
result = debug_cmd('message:What is 2+2?', session_id)

# Client commands (visual debug)
result = debug_cmd('client:enable', session_id)
result = debug_cmd('client:frame', session_id)

# Tester commands (spawn and control test instances)
result = debug_cmd('tester:spawn {"cwd":"/tmp"}', session_id)
result = debug_cmd('tester:list', session_id)
result = debug_cmd('tester:tester_abc123:frame', session_id)
```

### Selfdev Tool Actions

When in self-dev mode, the `selfdev` tool is available:

```python
# Check build status
debug_cmd('tool:selfdev {"action":"status"}', session_id)

# Spawn a test instance
debug_cmd('tool:selfdev {"action":"spawn-tester","cwd":"/tmp","args":["--help"]}', session_id)

# List testers
debug_cmd('tool:selfdev {"action":"tester","command":"list"}', session_id)

# Control tester
debug_cmd('tool:selfdev {"action":"tester","command":"stop","id":"tester_xxx"}', session_id)
```

### Known Issues

- **Claude provider**: The Claude model may claim it doesn't have access to the `selfdev` tool even when it's registered. Direct tool execution via debug socket works. GPT models correctly see and use selfdev.
