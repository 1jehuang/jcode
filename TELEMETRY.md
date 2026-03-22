# jcode Telemetry

jcode collects **anonymous, minimal usage statistics** to help understand how many people use jcode, what providers/models are popular, and whether sessions are succeeding or crashing. This data helps prioritize development without collecting prompts or code.

## What We Collect

### Install Event (sent once, on first launch)

| Field | Example | Purpose |
|-------|---------|----------|
| `id` | `a1b2c3d4-...` | Random UUID, not tied to your identity |
| `event` | `"install"` | Event type |
| `version` | `"0.6.0"` | jcode version |
| `os` | `"linux"` | Operating system |
| `arch` | `"x86_64"` | CPU architecture |

### Session Start Event

| Field | Example | Purpose |
|-------|---------|----------|
| `id` | `a1b2c3d4-...` | Same random UUID |
| `event` | `"session_start"` | Event type |
| `version` | `"0.6.0"` | jcode version |
| `os` | `"linux"` | Operating system |
| `arch` | `"x86_64"` | CPU architecture |
| `provider_start` | `"OpenAI"` | Provider when session started |
| `model_start` | `"gpt-5.4"` | Model when session started |
| `resumed_session` | `false` | Whether this was a resumed session |

### Session End / Crash Event

| Field | Example | Purpose |
|-------|---------|----------|
| `id` | `a1b2c3d4-...` | Same random UUID |
| `event` | `"session_end"` / `"session_crash"` | Event type |
| `version` | `"0.6.0"` | jcode version |
| `os` | `"linux"` | Operating system |
| `arch` | `"x86_64"` | CPU architecture |
| `provider_start` | `"OpenAI"` | Provider when session started |
| `provider_end` | `"OpenAI"` | Provider when session ended |
| `model_start` | `"gpt-5.4"` | Model when session started |
| `model_end` | `"gpt-5.4"` | Model when session ended |
| `provider_switches` | `0` | How many times you switched providers |
| `model_switches` | `1` | How many times you switched models |
| `duration_mins` | `45` | Session length in minutes |
| `turns` | `23` | Number of user prompts sent |
| `had_user_prompt` | `true` | Whether any real prompt was submitted |
| `had_assistant_response` | `true` | Whether the assistant produced a response |
| `assistant_responses` | `6` | Number of assistant responses |
| `tool_calls` | `8` | Number of tool executions |
| `tool_failures` | `1` | Number of tool execution failures |
| `transport_https` | `2` | Number of provider requests sent over HTTPS/SSE |
| `transport_persistent_ws_fresh` | `1` | Number of fresh persistent WebSocket requests |
| `transport_persistent_ws_reuse` | `5` | Number of turns that reused an existing persistent WebSocket |
| `transport_cli_subprocess` | `0` | Number of requests sent through a CLI subprocess transport |
| `transport_native_http2` | `0` | Number of requests sent through native HTTP/2 transports |
| `transport_other` | `0` | Number of requests using any other transport |
| `resumed_session` | `false` | Whether this session was resumed |
| `end_reason` | `"normal_exit"` | Coarse end reason |
| `errors` | `{"provider_timeout": 0, ...}` | Count of errors by category |

## What We Do NOT Collect

- No file paths, project names, or directory structures
- No code, prompts, or LLM responses
- No tool inputs or tool outputs
- No MCP server names or configurations
- No IP addresses (Cloudflare Workers don't log these by default)
- No personal information of any kind
- No error messages or stack traces in telemetry (only coarse categories and end reasons)

The UUID is randomly generated on first run and stored at `~/.jcode/telemetry_id`. It is not derived from your machine, username, email, or any identifiable information.

## How It Works

1. On first launch, jcode generates a random UUID and sends an `install` event
2. When a session begins, jcode sends a `session_start` event
3. When a session ends normally, jcode sends a `session_end` event with coarse session metrics
4. On best-effort crash/signal handling, jcode sends a `session_crash` event
5. Requests are fire-and-forget HTTP POSTs that don't block startup or shutdown
6. If a request fails (offline, firewall, etc.), jcode silently continues - no retries, no queuing

The telemetry endpoint is a Cloudflare Worker that stores events in a D1 database. The source code for the worker is in [`telemetry-worker/`](./telemetry-worker/).

## How to Opt Out

Any of these methods will disable telemetry completely:

```bash
# Option 1: Environment variable
export JCODE_NO_TELEMETRY=1

# Option 2: Standard DO_NOT_TRACK (https://consoledonottrack.com/)
export DO_NOT_TRACK=1

# Option 3: File-based opt-out
touch ~/.jcode/no_telemetry
```

When opted out, zero network requests are made. The telemetry module short-circuits immediately.

## Verification

This is open source. The entire telemetry implementation is in [`src/telemetry.rs`](./src/telemetry.rs) - you can read exactly what gets sent. There are no other network calls related to telemetry anywhere in the codebase.

## Data Retention

Telemetry data is used in aggregate only (install count, active users, provider distribution, session success/crash rates, feature-level counts). Individual event records are retained for up to 12 months and then deleted.
