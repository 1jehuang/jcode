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
kitten @ --to unix:$sock launch --type=os-window ./target/release/jcode
```

**Programmatic testing** - Use the debug socket for automated testing (see "Headless Testing via Debug Socket" section below).

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

## Authentication

jcode supports multiple providers and authentication methods:

### Option 1: OAuth via Claude Subscription (Recommended)

Uses your Claude Pro/Max subscription - no API key needed, included in subscription cost.

**Setup:**
1. Install Claude Code CLI: `npm install -g @anthropic-ai/claude-code`
2. Login: `claude login`
3. Credentials are stored in `~/.claude/.credentials.json`

jcode automatically detects and uses these credentials. Token refresh is handled automatically.

**How it works:**
- jcode reads the OAuth token from `~/.claude/.credentials.json`
- Sends requests to `api.anthropic.com/v1/messages?beta=true`
- Includes required headers: `anthropic-beta: oauth-2025-04-20,claude-code-20250219`
- System prompt must include Claude Code identity (handled automatically)

### Option 2: Direct API Key

Pay-per-token via Anthropic Console.

```bash
export ANTHROPIC_API_KEY=sk-ant-...
```

If `ANTHROPIC_API_KEY` is set, it takes priority over OAuth.

### Prompt Caching

Both auth methods support prompt caching. The system prompt is cached for 5 minutes, reducing costs on subsequent turns. Token usage shows:
- `cache_write`: Tokens cached on first request
- `cache_read`: Tokens read from cache (90% cheaper)

### Legacy: Claude CLI Provider

To use Claude Code CLI as a subprocess (legacy mode):

```bash
export JCODE_USE_CLAUDE_CLI=1
```

This shells out to `claude` binary instead of calling the API directly.

### Option 3: OpenRouter (200+ Models)

Access 200+ models from various providers (Anthropic, OpenAI, Google, Meta, etc.) via OpenRouter.

**Setup:**
1. Get an API key from https://openrouter.ai/
2. Set the environment variable:

```bash
export OPENROUTER_API_KEY=sk-or-v1-...
```

**Usage:**
- Models use `provider/model` format (e.g., `anthropic/claude-sonnet-4`, `openai/gpt-4o`)
- Switch models with `/model anthropic/claude-sonnet-4`
- Available models are fetched dynamically from OpenRouter API

**Features:**
- Unified API for multiple providers
- Pay-per-token pricing
- Automatic provider routing and fallbacks

## Environment Variables

```bash
# Authentication
export ANTHROPIC_API_KEY=sk-ant-...     # Direct API key (overrides OAuth)
export OPENROUTER_API_KEY=sk-or-v1-...  # OpenRouter API key
export JCODE_USE_CLAUDE_CLI=1           # Use Claude CLI subprocess (legacy)

# Model selection
export JCODE_ANTHROPIC_MODEL=claude-opus-4-5-20251101
export JCODE_OPENROUTER_MODEL=anthropic/claude-sonnet-4  # Default OpenRouter model

# Debugging
export JCODE_ANTHROPIC_DEBUG=1          # Log API request payloads
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
| `create_session:<path>` | Create session with working directory |
| `destroy_session:<id>` | Destroy a session |
| `set_model:<model>` | Switch model (may change provider) |
| `set_provider:<name>` | Switch provider (claude/openai/openrouter) |
| `trigger_extraction` | Force end-of-session memory extraction |
| `available_models` | List all available models |
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

**Swarm Commands** (multi-agent coordination - `swarm:` prefix):
| Command | Description |
|---------|-------------|
| `swarm` / `swarm:members` | List all swarm members with details (includes timestamps, provider/model) |
| `swarm:list` | List all swarm IDs with member counts |
| `swarm:info:<swarm_id>` | Full info: members, coordinator, plan, context, conflicts |
| `swarm:coordinators` | List all coordinators (swarm_id -> session_id) |
| `swarm:coordinator:<swarm_id>` | Get coordinator for specific swarm |
| `swarm:plans` | List all swarm plans with todo counts |
| `swarm:plan:<swarm_id>` | Get todos for specific swarm |
| `swarm:proposals` | List all pending plan proposals |
| `swarm:proposals:<swarm_id>` | List proposals for specific swarm (with items) |
| `swarm:proposals:<session_id>` | Get detailed proposal from a session |
| `swarm:context` | List all shared context entries (includes timestamps) |
| `swarm:context:<swarm_id>` | List context for specific swarm |
| `swarm:context:<swarm_id>:<key>` | Get specific context value |
| `swarm:touches` | List all file touches (path, session, op, age, timestamp_unix) |
| `swarm:touches:<path>` | Get touches for specific file |
| `swarm:touches:swarm:<swarm_id>` | Get touches filtered by swarm members |
| `swarm:conflicts` | Files touched by multiple sessions (with full access history) |
| `swarm:session:<id>` | Detailed session state (interrupts, provider, token usage) |
| `swarm:interrupts` | List pending interrupts across all sessions |
| `swarm:id:<path>` | Compute swarm_id for a path (shows git_root, is_git_repo) |
| `swarm:broadcast:<message>` | Broadcast message to all swarm members |
| `swarm:broadcast:<swarm_id> <message>` | Broadcast to specific swarm |
| `swarm:notify:<session_id> <message>` | Send DM to specific session |
| `swarm:help` | Full swarm command reference |

**Event Commands** (real-time event subscription - `events:` prefix):
| Command | Description |
|---------|-------------|
| `events:recent` | Get recent 50 events |
| `events:recent:<N>` | Get recent N events |
| `events:since:<id>` | Get events since event ID (for polling) |
| `events:count` | Get event count and latest ID |
| `events:types` | List available event types |

Event types include: `file_touch`, `notification`, `plan_update`, `plan_proposal`, `context_update`, `status_change`, `member_change`. Events include timestamps (`age_secs`, `timestamp_unix`) for debugging timing issues.

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

### Testing with Multiple Providers

Use the debug socket to test features with both Claude and OpenAI:

```python
import socket
import json

def send_cmd(sock, cmd, session_id=None, timeout=60):
    req = {"type": "debug_command", "id": 1, "command": cmd}
    if session_id:
        req["session_id"] = session_id
    sock.send((json.dumps(req) + '\n').encode())
    sock.settimeout(timeout)
    data = sock.recv(65536).decode()
    resp = json.loads(data)
    return resp.get('ok'), resp.get('output', '')

sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
sock.connect('/run/user/1000/jcode-debug.sock')

# Create a test session
ok, output = send_cmd(sock, "create_session:/tmp/test")
session_id = json.loads(output)['session_id']

# Test with Claude
send_cmd(sock, "set_provider:claude", session_id)
send_cmd(sock, "message:Remember I prefer dark mode", session_id)

# Test with OpenAI
send_cmd(sock, "set_provider:openai", session_id)
send_cmd(sock, "message:What are my preferences?", session_id)

# Test memory extraction
send_cmd(sock, "trigger_extraction", session_id)

# Check memories
send_cmd(sock, 'tool:memory {"action":"list"}', session_id)

# Cleanup
send_cmd(sock, f"destroy_session:{session_id}")
sock.close()
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
