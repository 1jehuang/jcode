# Non-Interactive Orchestrator API

Status: draft integration contract.

This document describes the programmatic surfaces an external orchestrator can use
today and the stable contract jcode should expose for long-lived multi-agent
control. The goal is to keep orchestrators off the TUI and off terminal scraping.

## Current Surfaces

### One-Shot Headless Runs

`jcode run` is the supported non-interactive entry point for a single prompt.

```bash
jcode --provider claude -C /workspace/project run --ndjson "Fix the failing test"
jcode --provider-profile my-api -m my-model run --json "Summarize this repo"
jcode --resume session_red_fox_123 run --ndjson "Continue"
```

`--ndjson` emits one JSON object per line on stdout. Important event types:

```json
{"type":"start","session_id":"session_red_fox_123","provider":"claude","model":"claude-sonnet-4-6"}
{"type":"text_delta","text":"..."}
{"type":"tool_start","id":"toolu_...","name":"read"}
{"type":"tool_input","delta":"..."}
{"type":"tool_exec","id":"toolu_...","name":"bash"}
{"type":"tool_done","id":"toolu_...","name":"bash","output":"...","error":false}
{"type":"tokens","input":1000,"output":200,"cache_read_input":null,"cache_creation_input":null}
{"type":"compaction","trigger":"auto","pre_tokens":120000,"messages_dropped":42,"post_tokens":30000,"tokens_saved":90000,"duration_ms":850}
{"type":"done","session_id":"session_red_fox_123","provider":"claude","model":"claude-sonnet-4-6","text":"...","usage":{"input_tokens":1000,"output_tokens":200,"cache_read_input_tokens":null,"cache_creation_input_tokens":null}}
```

`--json` emits a single final object:

```json
{
  "session_id": "session_red_fox_123",
  "provider": "claude",
  "model": "claude-sonnet-4-6",
  "text": "...",
  "usage": {
    "input_tokens": 1000,
    "output_tokens": 200,
    "cache_read_input_tokens": null,
    "cache_creation_input_tokens": null
  }
}
```

This surface creates or resumes a persisted session, but it does not currently
expose daemon attach/detach or a durable active-session lease.

### Daemon JSON Socket

The jcode server protocol is newline-delimited JSON over the main socket. The
Rust types live in `crates/jcode-protocol/src/lib.rs`.

Useful requests:

```json
{"type":"subscribe","id":1,"working_dir":"/workspace/project","target_session_id":"session_red_fox_123","client_has_local_history":false,"allow_session_takeover":true}
{"type":"resume_session","id":2,"session_id":"session_red_fox_123","client_has_local_history":false,"allow_session_takeover":true}
{"type":"get_history","id":3}
{"type":"message","id":4,"content":"Continue from here","images":[]}
{"type":"cancel","id":5}
```

Useful server events:

```json
{"type":"ack","id":4}
{"type":"history","id":3,"session_id":"session_red_fox_123","messages":[],"provider_name":"claude","provider_model":"claude-sonnet-4-6","mcp_servers":["filesystem"],"all_sessions":["session_red_fox_123"],"client_count":1}
{"type":"session","session_id":"session_red_fox_123"}
{"type":"text_delta","text":"..."}
{"type":"token_usage","input":1000,"output":200,"cache_read_input":50000,"cache_creation_input":null}
{"type":"done","id":4}
{"type":"error","id":4,"message":"...","retry_after_secs":30}
```

The daemon socket is the right technical base for a stable orchestrator API, but
the current protocol is still client/TUI-shaped. Treat it as an internal protocol
unless both sides are pinned to a specific jcode build.

### Debug Socket

The debug socket has JSON commands that are useful for early integrations:

```bash
jcode debug start
jcode debug sessions
jcode debug create_session:D:\work\project
jcode debug -S session_red_fox_123 message_async:Run the next task
jcode debug events:subscribe
```

Equivalent debug request:

```json
{"type":"debug_command","id":1,"command":"create_session:/workspace/project"}
```

Responses are wrapped as:

```json
{"type":"debug_response","id":1,"ok":true,"output":"{\"session_id\":\"session_red_fox_123\",\"working_dir\":\"/workspace/project\",\"swarm_id\":\"/workspace/project\",\"friendly_name\":\"red_fox\",\"is_canary\":false}"}
```

This is not a stable public API. It exists for diagnostics and self-development,
and command names or payloads may change.

## Proposed Stable API

Expose this contract as `jcode api ...` CLI commands and the same JSON messages
over a versioned daemon endpoint. CLI output should be JSON by default, with
NDJSON for streaming commands.

Every JSON response should include:

```json
{
  "api_version": 1,
  "jcode_version": "v0.9.1888-dev",
  "ok": true
}
```

Errors should be structured and should never require stderr parsing:

```json
{
  "api_version": 1,
  "ok": false,
  "error": {
    "code": "session_not_found",
    "message": "Unknown session_id 'session_missing'",
    "retry_after_secs": null
  }
}
```

### Spawn

Command:

```bash
jcode api spawn \
  --provider claude \
  --model claude-sonnet-4-6 \
  --cwd /workspace/project \
  --prompt "Implement issue 123" \
  --detach
```

Request:

```json
{
  "type": "api.spawn",
  "id": 1,
  "provider": "claude",
  "provider_profile": null,
  "model": "claude-sonnet-4-6",
  "cwd": "/workspace/project",
  "initial_prompt": "Implement issue 123",
  "detached": true,
  "metadata": {
    "orchestrator": "octogent",
    "job_id": "job_123"
  }
}
```

Response:

```json
{
  "api_version": 1,
  "ok": true,
  "session_id": "session_red_fox_123",
  "provider": "claude",
  "model": "claude-sonnet-4-6",
  "cwd": "/workspace/project",
  "status": "running",
  "attached": false,
  "mcp_servers": ["filesystem"]
}
```

Semantics:

- `--detach` starts the turn and returns after the session is accepted.
- Without `--detach`, the command streams events until the first turn completes.
- Provider selection accepts either `--provider` or `--provider-profile`; profile
  behavior matches the existing top-level flags.
- `cwd` is persisted as the session working directory.

### Attach

Command:

```bash
jcode api attach session_red_fox_123 --ndjson
```

Request:

```json
{"type":"api.attach","id":2,"session_id":"session_red_fox_123","replay_history":true}
```

Response stream:

```json
{"type":"attached","session_id":"session_red_fox_123","provider":"claude","model":"claude-sonnet-4-6","cwd":"/workspace/project"}
{"type":"history","session_id":"session_red_fox_123","messages":[]}
{"type":"text_delta","text":"..."}
{"type":"done","session_id":"session_red_fox_123","turn_id":"turn_456"}
```

Attach is non-exclusive by default. If an orchestrator needs exclusive control,
it should request a lease:

```json
{"type":"api.attach","id":2,"session_id":"session_red_fox_123","lease":"exclusive","lease_ttl_secs":60}
```

### Detach

Command:

```bash
jcode api detach session_red_fox_123
```

Request:

```json
{"type":"api.detach","id":3,"session_id":"session_red_fox_123","lease_id":"lease_abc"}
```

Response:

```json
{"api_version":1,"ok":true,"session_id":"session_red_fox_123","status":"running"}
```

Detach closes the client stream and releases any lease. It must not stop the
session unless `stop` is requested separately.

### Stop

Command:

```bash
jcode api stop session_red_fox_123 --graceful
```

Request:

```json
{"type":"api.stop","id":4,"session_id":"session_red_fox_123","mode":"graceful"}
```

`mode` values:

- `graceful`: cancel current work at the next safe point, persist transcript, run
  end-of-session memory extraction if enabled.
- `force`: drop the live session immediately after persisting the current state
  best-effort.

### Session Inventory

Command:

```bash
jcode api sessions
```

Response:

```json
{
  "api_version": 1,
  "ok": true,
  "sessions": [
    {
      "session_id": "session_red_fox_123",
      "provider": "claude",
      "provider_profile": null,
      "model": "claude-sonnet-4-6",
      "cwd": "/workspace/project",
      "status": "running",
      "attached_clients": 1,
      "is_headless": true,
      "created_at": "2026-05-07T10:00:00Z",
      "last_active_at": "2026-05-07T10:04:12Z",
      "resumable": true,
      "resume_source": "jcode",
      "token_usage": {
        "input": 1000,
        "output": 200,
        "cache_read_input": 50000,
        "cache_creation_input": null
      },
      "cache": {
        "provider": "anthropic",
        "ttl_secs": 300,
        "last_write_at": "2026-05-07T10:04:10Z",
        "expires_at": "2026-05-07T10:09:10Z",
        "state": "warm"
      },
      "mcp_servers": ["filesystem"]
    }
  ]
}
```

Inventory must include:

- live daemon sessions
- persisted jcode sessions that can be resumed
- imported/resumable external harness sessions when discoverable

`status` values should be one of `idle`, `running`, `detached`, `stopped`,
`failed`, `crashed`, or `unknown`.

### Resume By ID

Command:

```bash
jcode api resume session_red_fox_123 --ndjson
jcode api resume codex:abc123 --cwd /workspace/project --ndjson
jcode api resume claude-code:abc123 --cwd /workspace/project --ndjson
jcode api resume opencode:abc123 --cwd /workspace/project --ndjson
jcode api resume pi:abc123 --cwd /workspace/project --ndjson
```

Request:

```json
{
  "type": "api.resume",
  "id": 5,
  "session_id": "codex:abc123",
  "source": "codex",
  "cwd": "/workspace/project",
  "attach": true
}
```

Response:

```json
{
  "api_version": 1,
  "ok": true,
  "session_id": "session_red_fox_123",
  "source": "codex",
  "source_session_id": "abc123",
  "provider": "openai",
  "model": "gpt-5.5",
  "cwd": "/workspace/project",
  "attached": true
}
```

The interactive `/resume` picker already advertises cross-harness resume for
Codex, Claude Code, OpenCode, and pi. The stable API should expose the same
import/resume path without requiring the picker. Until then, the only stable CLI
resume contract is `jcode --resume <jcode-session-id> run ...` for jcode-owned
session IDs.

### Streaming I/O

Streaming command:

```bash
jcode api send session_red_fox_123 --ndjson "Continue"
```

Events should be NDJSON. Each event should include `session_id`; turn-scoped
events should also include `turn_id`.

Required event types:

```json
{"type":"turn_started","session_id":"session_red_fox_123","turn_id":"turn_456"}
{"type":"stdout","session_id":"session_red_fox_123","turn_id":"turn_456","stream":"assistant","text":"..."}
{"type":"stderr","session_id":"session_red_fox_123","turn_id":"turn_456","text":"provider warning"}
{"type":"tool_start","session_id":"session_red_fox_123","turn_id":"turn_456","tool_call_id":"toolu_1","name":"bash"}
{"type":"tool_input","session_id":"session_red_fox_123","turn_id":"turn_456","tool_call_id":"toolu_1","delta":"..."}
{"type":"tool_done","session_id":"session_red_fox_123","turn_id":"turn_456","tool_call_id":"toolu_1","name":"bash","output":"...","error":false}
{"type":"token_usage","session_id":"session_red_fox_123","turn_id":"turn_456","input":1000,"output":200,"cache_read_input":50000,"cache_creation_input":null}
{"type":"turn_done","session_id":"session_red_fox_123","turn_id":"turn_456","status":"ok"}
```

The daemon may also expose a websocket, but NDJSON over stdout and the local
socket is the baseline contract because it is easy to supervise from CI, IDEs,
and process managers.

### Cache-Cold Signal

The TUI tracks the Anthropic cache TTL and warns when a session has gone cold.
That signal should be emitted structurally:

```json
{
  "type": "cache_state",
  "session_id": "session_red_fox_123",
  "provider": "anthropic",
  "model": "claude-sonnet-4-6",
  "state": "cold",
  "ttl_secs": 300,
  "last_cache_write_at": "2026-05-07T10:04:10Z",
  "expires_at": "2026-05-07T10:09:10Z",
  "reason": "ttl_expired"
}
```

`state` values:

- `unknown`: no provider cache telemetry yet
- `warm`: cache is expected to be reusable
- `cooling`: cache has less than 60 seconds before expiry
- `cold`: cache is expired or provider/model changed
- `missed`: provider reported an unexpected cache miss

Schedulers should use `cold` and `cooling` to avoid waking Anthropic sessions
after the 5-minute default TTL unless the work is worth the cache miss. When the
user enables the 1-hour Anthropic cache TTL, `ttl_secs` should report `3600`.

### MCP Passthrough

Headless sessions should load the same MCP configuration as TUI sessions:

- global `~/.jcode/mcp.json`
- project-local `.jcode/mcp.json`
- project-local `.claude/mcp.json` compatibility fallback
- first-run import from `~/.claude/mcp.json` and `~/.codex/config.toml` when
  `~/.jcode/mcp.json` does not exist yet

Daemon-mode headless sessions use the shared MCP pool for servers with
`"shared": true` and per-session clients for `"shared": false`. Inventory and
attach/history responses should include the connected `mcp_servers` list so an
orchestrator can verify tool availability.

## Compatibility Promise

For API version 1:

- Existing fields keep their meaning.
- New fields may be added.
- Enum values may be added; clients should ignore unknown values.
- Event ordering is stable within a single session stream.
- JSON events are UTF-8, one object per line, and never mixed with human text on
  stdout.
- Human diagnostics go to stderr only when the selected output mode is JSON or
  NDJSON.

## Implementation Gaps

These are the pieces not yet covered by a stable public surface:

- `jcode api spawn/attach/detach/stop` command group.
- Session inventory that combines live sessions, detached headless sessions, and
  persisted resumable sessions with last-active timestamps.
- Public cross-harness resume CLI/JSON contract for Codex, Claude Code,
  OpenCode, and pi.
- Structured cache-state events outside the TUI.
- Stable lease semantics for external orchestrators.
- Explicit MCP server list in the proposed inventory payload.

