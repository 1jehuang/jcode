# Structured runtime commands

The harness API advertises `runtime_commands` only when the attached daemon's safe
capability probe advertises it. Older daemons return an explicit unsupported error.
This is additive API v1.10, available in both the Rust and TypeScript SDKs.

```rust,no_run
use jcode_sdk::{JcodeClient, RuntimeCommand};
# fn example(client: &JcodeClient, session: &str) -> jcode_sdk::Result<()> {
let result = client.runtime_command(session, RuntimeCommand::Memory, vec!["status".into()])?;
println!("{}", result.message);
# Ok(()) }
```

```json
{"v":1,"id":7,"req":"runtime_command","session_id":"session-id","command":"memory","args":["status"]}
{"v":1,"reply_to":7,"ev":"runtime_command_result","session_id":"session-id","command":"memory","data":{"enabled":true},"message":"..."}
```

Rust events use direct `RuntimeCommandResult { session_id, command, data, message }`
fields. The Rust SDK returns `RuntimeCommandOutput`. TypeScript exposes
`runtimeCommand(sessionId, command, args)` with equivalent fields. `data` is
command-specific and additive. `message` is suitable for display. Commands are
never converted into generic agent prompts. State is read and changed on the
session's runtime host, including SSH and managed remote runtimes.

## Available behavior

- `skills`: loaded global/project-overlay skills, descriptions, paths, active
  skill, and shared endorsed recommendations. Refreshes the global registry.
- `context`: assembled static/dynamic system prompt, effective/current tools,
  active conversation messages and context-window metadata. Does not consume
  pending memory, prewarm a provider, or run inference.
- `info`, `cache stats`: real provider/session identity and token/cache counters.
  Cache expiry/content inspection is unavailable and marked explicitly.
- `cache extend`, `cache 1h`, `cache 5m`: persist Anthropic cache TTL for future
  requests. They do not change an already-written cache entry.
- `memory`: toggle, explicit on/off/status, or schedule `extract`. Extraction
  replies mean scheduled, not completed.
- `swarm`: status/on/off/toggle using native membership management.
- `agents [global] [swarm|review|judge|memory|ambient] [model|inherit]`: read
  defaults and session overrides, or persist the requested policy scope.
- `subagent_model`: show/set/inherit the spawned-worker policy.
- `autoreview`, `autojudge`: show status and persist explicit on/off/toggle.
- `transport`: status or provider-validated auto/https/websocket.
- `fast`: bare command toggles priority/off, explicit on/off/flex/status supported.
  `ultra`, `ultrafast`, and `ultra-fast` explicitly select Ultrafast on supported
  OpenAI models. See [OpenAI Ultrafast](OPENAI_ULTRAFAST.md) for opt-in costs
  and API versus Codex access restrictions.
  Providers that cannot apply it return their actual error.
- `telemetry`: show status or persist everything/no-prompts/nothing, honoring
  environment-forced opt-out.
- `subscription`: actual live hosted account usage when signed in. No credentials
  are returned. Network/auth failures are errors, not invented status.
- `provider_test_coverage`: persisted coverage report, never a paid live probe.
- `refresh_model_list`: actual provider catalog refresh.
- `continue`, `resume_all`: native continuation of interrupted live sessions,
  leaving busy/completed/fresh sessions alone.
- `review`, `judge`, `subagent`: actual independent swarm-worker launch using
  session/global model policy, returning structured spawned-session metadata.
  `--model` and `--type` are supported. The deprecated direct subagent tool stays
  unregistered. `--continue` is unavailable here and reports an explicit error.
- `overnight`: native supervised child run plus session-scoped status/log/review/
  cancel controls. `productivity`: real report and image payload on the runtime
  host. `initiatives`: honest retirement explanation, never re-enabled.
- `premium`: actual supported provider conservation setting.
- `remote`: native gateway status/on/off/pair/revoke/help. Config toggles report
  restart requirements rather than pretending a listener changed immediately.
- `server_reload`: requests a non-forced, state-preserving upgrade only if a
  newer binary is available. Initial reply is request acceptance, not completion.
- `auth`, `logout`: runtime-host auth inspection and explicitly confirmed
  provider credential removal, subject to supported shared auth stores.

`transfer_session` is a separate lifecycle operation, exposed as
`transferSession` / `transfer_session`. It creates a real runtime-compacted child
and replies `SessionForked`. It does not attach the caller to the child.

## Limitations and deployment

Most agent-backed reads/mutations return a busy error during an active model turn,
rather than waiting behind an arbitrarily long generation. Provider-specific
settings return errors if unsupported. Cloud UI setup, cache eviction/prewarming,
and deprecated initiative tracking are not falsely advertised as working.

A new SDK or bridge does not upgrade a running daemon. Build the runtime and
bridge, install the new runtime candidate, then use the existing graceful server
upgrade workflow. Preserve session state and do not kill a live daemon or the
Desktop harness bridge. Desktop bridge upgrades use its `reload-bridge` workflow.
Until both sides run the new code, the capability remains unavailable.
