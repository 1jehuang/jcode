# jcode Telemetry

jcode collects **anonymous, minimal usage statistics** to help understand how many people use jcode, what providers/models are popular, whether onboarding works, which feature families are used, how often sessions succeed, and whether performance/regressions are improving. This data helps prioritize development without collecting prompts or code.

Recent telemetry additions also include: coarse onboarding steps, explicit thumbs-up / thumbs-down feedback, build-channel / dev-mode cleanup flags, session/workflow/tool-category summaries, coarse project language buckets, retention helpers like active days in the last 7 / 30 days, workflow cadence fields for session timing and multi-sessioning, privacy-safe per-turn timing/outcome metrics, and schema v5 agent-time / autonomy / pain-attribution metrics.

## What We Collect

### Install Event (sent once, on first launch)

| Field | Example | Purpose |
|-------|---------|----------|
| `id` | `a1b2c3d4-...` | Random UUID, not tied to your identity |
| `event` | `"install"` | Event type |
| `version` | `"0.6.0"` | jcode version |
| `os` | `"linux"` | Operating system |
| `arch` | `"x86_64"` | CPU architecture |

### Upgrade Event

| Field | Example | Purpose |
|-------|---------|----------|
| `id` | `a1b2c3d4-...` | Same random UUID |
| `event` | `"upgrade"` | Event type |
| `version` | `"0.9.1"` | Current jcode version |
| `from_version` | `"0.8.1"` | Previously recorded jcode version |
| `os` / `arch` | `"linux"` / `"x86_64"` | Environment breakdown |

### Auth Success Event

| Field | Example | Purpose |
|-------|---------|----------|
| `id` | `a1b2c3d4-...` | Same random UUID |
| `event` | `"auth_success"` | Event type |
| `auth_provider` | `"claude"` | Which provider/account system was configured |
| `auth_method` | `"oauth"` | Coarse auth method only |
| `version` / `os` / `arch` | `"0.9.1"` / `"linux"` / `"x86_64"` | Activation funnel dimensions |

### Onboarding Step Event

| Field | Example | Purpose |
|-------|---------|----------|
| `event` | `"onboarding_step"` | Event type |
| `step` | `"first_prompt_sent"` | Coarse funnel step |
| `auth_provider` | `"openai"` | Optional provider dimension for auth steps |
| `auth_method` | `"oauth"` | Optional auth-method dimension for auth steps |
| `milestone_elapsed_ms` | `42000` | Rough time from install to milestone |

### Feedback Event

| Field | Example | Purpose |
|-------|---------|----------|
| `event` | `"feedback"` | Event type |
| `feedback_rating` | `"up"` / `"down"` | Explicit product sentiment |
| `feedback_reason` | `"slow"` | Optional coarse reason bucket |

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
| `session_start_hour_utc` | `13` | Coarse hour-of-day bucket for workflow timing |
| `session_start_weekday_utc` | `2` | Weekday bucket for usage cadence |
| `previous_session_gap_secs` | `3600` | How long since this install's previous session |
| `sessions_started_24h` / `sessions_started_7d` | `3` / `8` | How bursty a user's workflow is recently |
| `active_sessions_at_start` | `2` | Concurrent sessions observed including this one |
| `other_active_sessions_at_start` | `1` | Other sessions already open when this started |

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
| `duration_secs` | `2700` | Finer-grained session length |
| `turns` | `23` | Number of user prompts sent |
| `had_user_prompt` | `true` | Whether any real prompt was submitted |
| `had_assistant_response` | `true` | Whether the assistant produced a response |
| `assistant_responses` | `6` | Number of assistant responses |
| `first_assistant_response_ms` | `1200` | Time to first assistant response |
| `first_tool_call_ms` | `900` | Time to first tool invocation |
| `first_tool_success_ms` | `1500` | Time to first successful tool execution |
| `first_file_edit_ms` | `2200` | Time to first successful file edit |
| `first_test_pass_ms` | `4100` | Time to first successful test run |
| `tool_calls` | `8` | Number of tool executions |
| `tool_failures` | `1` | Number of tool execution failures |
| `executed_tool_calls` | `10` | Centralized count of actual registry tool executions |
| `executed_tool_successes` | `9` | Successful registry tool executions |
| `executed_tool_failures` | `1` | Failed registry tool executions |
| `tool_latency_total_ms` | `4200` | Aggregate tool execution latency |
| `tool_latency_max_ms` | `1800` | Slowest single tool call |
| `file_write_calls` | `2` | Count of write/edit/patch style tool uses |
| `tests_run` | `1` | Coarse count of test runs triggered |
| `tests_passed` | `1` | Coarse count of successful test runs |
| `input_tokens` / `output_tokens` | `12345` / `678` | Session-level provider-reported token usage totals |
| `cache_read_input_tokens` / `cache_creation_input_tokens` | `9000` / `1200` | Session-level provider-reported prompt-cache token totals when available |
| `total_tokens` | `23223` | Sum of input, output, cache-read, and cache-creation tokens |
| `feature_*_used` | `true/false` | Whether a feature family was used (memory, swarm, web, email, MCP, side panel, goals, selfdev, background, subagents) |
| `tool_cat_*` | `0..N` | Coarse tool category counts (read/search, write, shell, web, memory, subagent, swarm, email, side-panel, goal, MCP, other) |
| `command_*_used` | `true/false` | Whether a slash-command family was used in-session |
| `workflow_*_used` | `true/false` | Whether the session looked like coding, research, testing, background, subagent, or swarm work |
| `unique_mcp_servers` | `2` | Count of distinct MCP servers touched in-session |
| `session_success` | `true` | Coarse success proxy based on outcomes like responses, successful tools, tests, or edits |
| `abandoned_before_response` | `false` | Whether the user engaged but got no successful outcome before ending |
| `session_stop_reason` | `"tool_error_loop"` | Coarse inferred pain/churn bucket, such as crash, auth blocked, rate limited, no first response, too slow, tool failures, no useful action, or completed successfully |
| `agent_role` | `"foreground"` / `"subagent"` | Coarse role classification for the session: foreground, background, subagent, or swarm |
| `parent_session_id` | `"session_..."` | Optional parent session ID for attributing spawned/background/subagent work to the initiating session |
| `agent_active_ms_total` | `7200000` | Sum of active agent time across finalized turns; two agents active for two hours count as four agent-hours in aggregate |
| `agent_model_ms_total` / `agent_tool_ms_total` | `5400000` / `1800000` | Approximate active-time split between model/agent thinking and registry tool execution latency |
| `session_idle_ms_total` | `300000` | Time around turns where the session was open but no agent activity was observed |
| `time_to_first_agent_action_ms` | `900` | Time from session start to the first assistant response or tool action |
| `time_to_first_useful_action_ms` | `1500` | Time from session start to the first successful tool/file/test outcome, falling back to first response |
| `spawned_agent_count` | `3` | Count of background, subagent, and swarm task invocations attributed to the session |
| `background_task_count` / `background_task_completed_count` | `1` / `1` | Background work started and successfully completed via background/scheduled tool paths |
| `subagent_task_count` / `subagent_success_count` | `1` / `1` | Subagent task invocations and successful completions |
| `swarm_task_count` / `swarm_success_count` | `1` / `0` | Swarm/agent-coordination task invocations and successful completions |
| `user_cancelled_count` | `1` | Urgent interrupt count, used to detect sessions where the user stopped the agent mid-work |
| `transport_https` | `2` | Number of provider requests sent over HTTPS/SSE |
| `transport_persistent_ws_fresh` | `1` | Number of fresh persistent WebSocket requests |
| `transport_persistent_ws_reuse` | `5` | Number of turns that reused an existing persistent WebSocket |
| `transport_cli_subprocess` | `0` | Number of requests sent through a CLI subprocess transport |
| `transport_native_http2` | `0` | Number of requests sent through native HTTP/2 transports |
| `transport_other` | `0` | Number of requests using any other transport |
| `project_repo_present` | `true` | Whether the working directory looked like a repo |
| `project_lang_*` | `true/false` | Coarse project-language buckets (Rust, JS/TS, Python, Go, Markdown, mixed) |
| `days_since_install` | `12` | Rough install age in days |
| `active_days_7d` / `active_days_30d` | `4` / `9` | How many distinct active days this install had recently |
| `session_start_hour_utc` / `session_end_hour_utc` | `13` / `14` | Session timing buckets for workflow analysis |
| `session_start_weekday_utc` / `session_end_weekday_utc` | `2` / `2` | Weekday timing buckets |
| `previous_session_gap_secs` | `1800` | Time since the previous session on this install |
| `sessions_started_24h` / `sessions_started_7d` | `5` / `12` | Recent session burstiness |
| `active_sessions_at_start` / `other_active_sessions_at_start` | `2` / `1` | Concurrent-session snapshot at session start |
| `max_concurrent_sessions` | `3` | Highest concurrent session count seen during the session |
| `multi_sessioned` | `true` | Whether the user appeared to be running multiple sessions |
| `resumed_session` | `false` | Whether this session was resumed |
| `end_reason` | `"normal_exit"` | Coarse end reason |
| `errors` | `{"provider_timeout": 0, ...}` | Count of errors by category |

### Turn End Event

This is a privacy-safe per-prompt summary event. It contains no prompt text, no response text, and no tool inputs/outputs.

| Field | Example | Purpose |
|-------|---------|----------|
| `event` | `"turn_end"` | Event type |
| `turn_index` | `4` | Which user turn in the session this was |
| `turn_started_ms` | `182000` | Time from session start to turn start |
| `turn_active_duration_ms` | `8200` | Active duration until the last meaningful response/tool activity |
| `idle_before_turn_ms` / `idle_after_turn_ms` | `45000` / `12000` | Workflow pacing around the turn |
| `assistant_responses` | `1` | Responses produced during this turn |
| `first_assistant_response_ms` | `1200` | Time to first response within the turn |
| `first_tool_call_ms` / `first_tool_success_ms` | `900` / `1500` | Tool timing within the turn |
| `first_file_edit_ms` / `first_test_pass_ms` | `2200` / `4100` | Useful outcome timing within the turn |
| `tool_calls` / `tool_failures` | `3` / `1` | Coarse tool activity within the turn |
| `executed_tool_calls` / `executed_tool_successes` / `executed_tool_failures` | `4` / `3` / `1` | Registry tool execution outcomes |
| `tool_latency_total_ms` / `tool_latency_max_ms` | `2600` / `1400` | Tool latency footprint within the turn |
| `file_write_calls` / `tests_run` / `tests_passed` | `1` / `1` / `1` | Outcome proxies for coding workflows |
| `input_tokens` / `output_tokens` | `1200` / `180` | Turn-level provider-reported token usage totals |
| `cache_read_input_tokens` / `cache_creation_input_tokens` | `8000` / `600` | Turn-level provider-reported prompt-cache token totals when available |
| `total_tokens` | `9980` | Sum of input, output, cache-read, and cache-creation tokens for the turn |
| `feature_*_used` | `true/false` | Which feature families were touched in the turn |
| `tool_cat_*` | `0..N` | Tool category mix for the turn |
| `workflow_*_used` | `true/false` | What kind of workflow this turn looked like |
| `turn_success` | `true` | Whether the turn produced a useful response/outcome |
| `turn_abandoned` | `false` | Whether the turn appears to have ended without success |
| `turn_end_reason` | `"next_user_prompt"` | Why the turn was finalized |

### Shared Event Metadata

Most events also carry a few coarse quality / cleanup fields:

| Field | Example | Purpose |
|-------|---------|----------|
| `event_id` | `"uuid"` | Deduplication |
| `session_id` | `"uuid"` | Joins session-scoped events together |
| `schema_version` | `3` | Forward-compatible parsing |
| `build_channel` | `"release"` / `"selfdev"` / `"local_build"` | Filter out dev/test usage |
| `is_git_checkout` | `true/false` | Distinguish source-tree usage from installed usage |
| `is_ci` | `true/false` | Filter CI noise |
| `ran_from_cargo` | `true/false` | Filter local dev launches |

## What We Do NOT Collect

- No file paths, project names, or directory structures
- No code, prompts, or LLM responses
- No tool inputs or tool outputs
- No MCP server names or configurations
- No IP addresses (Cloudflare Workers don't log these by default)
- No personal information of any kind
- No error messages or stack traces in telemetry (only coarse categories and end reasons)
- No exact wall-clock timestamps beyond coarse hour-of-day / weekday buckets

The UUID is randomly generated on first run and stored at `~/.jcode/telemetry_id`. It is not derived from your machine, username, email, or any identifiable information.

## How It Works

1. On first launch, jcode generates a random UUID and sends an `install` event
2. When a session begins, jcode sends a `session_start` event
3. When a session ends normally, jcode sends a `session_end` event with coarse session metrics
4. When auth succeeds, jcode sends a coarse `auth_success` event for activation-funnel analysis
5. When jcode detects a version change, it sends an `upgrade` event
6. On best-effort crash/signal handling, jcode sends a `session_crash` event
7. jcode may also send one-off onboarding milestone events and explicit feedback events when triggered
8. Requests are fire-and-forget HTTP POSTs that don't block normal usage (install/session shutdown have short bounded blocking timeouts)
9. If a request fails (offline, firewall, etc.), jcode silently continues - no retries, no queuing

The telemetry endpoint is a Cloudflare Worker that stores events in a D1 database. The source code for the worker is in [`telemetry-worker/`](./telemetry-worker/).

### Schema v5 deployment note

Agent-time, autonomy, and pain-attribution fields require the D1 migration in `telemetry-worker/migrations/0008_agent_time_and_churn.sql`. Until that migration is applied, schema v5 clients can still send the new JSON payloads, but the worker will drop unknown columns through dynamic column filtering and dashboard agent-time panels will remain empty or show optional-panel errors. After migration, run/redeploy the telemetry worker and query the dashboard's **Agent time / autonomy** panel.

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
