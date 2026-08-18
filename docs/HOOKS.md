# Lifecycle Hooks

jcode can run external commands at well-defined lifecycle points so other
programs can observe or gate agent behavior without forking jcode. Hooks
complement the [spawn hook](SPAWN_HOOK.md) (which controls *where headed
sessions appear*); lifecycle hooks tell you *what is happening inside them*.

## Configuration

```toml
# ~/.jcode/config.toml
[hooks]
turn_end      = "~/bin/jcode-turn-notify"     # observer
session_start = ""                            # observer
session_end   = ""                            # observer
pre_tool      = "~/bin/jcode-tool-policy"     # gate
post_tool     = ""                            # observer
pre_tool_timeout_ms = 5000
```

Env overrides (always win; empty value disables a config hook):
`JCODE_HOOK_TURN_END`, `JCODE_HOOK_SESSION_START`, `JCODE_HOOK_SESSION_END`,
`JCODE_HOOK_PRE_TOOL`, `JCODE_HOOK_POST_TOOL`, `JCODE_HOOK_PRE_TOOL_TIMEOUT_MS`.

## Common contract

- The hook command line is parsed shell-style (quotes and backslash escapes
  work) but executed **directly**, not through a shell. A leading `~/` in the
  program path is expanded.
- The hook runs in the session working directory when known.
- Every hook receives:

| Variable | Meaning |
| --- | --- |
| `JCODE_HOOK_EVENT` | `turn_end`, `session_start`, `session_end`, `pre_tool`, `post_tool` |
| `JCODE_HOOK_SESSION_ID` | Session the event belongs to |
| `JCODE_HOOK_CWD` | Session working directory |
| `JCODE_HOOK_PAYLOAD` | JSON object mirroring all fields (capped at 16 KB) |
| `JCODE_HOOKS_DISABLED` | Always `1`; suppresses hooks in nested jcode calls (recursion guard) |

## Observer hooks

`turn_end`, `session_start`, `session_end`, and `post_tool` are
**observers**: spawned detached, fire-and-forget. They can never block or slow
the agent; failures are only logged.

### `turn_end`

Fires when an agent turn completes (streaming turn path, which covers TUI,
desktop, swarm workers, and headless sessions).

Extra fields: `JCODE_HOOK_STATUS` (`ok`/`error`), `JCODE_HOOK_DURATION_MS`,
`JCODE_HOOK_MODEL`, `JCODE_HOOK_LAST_ASSISTANT_TEXT` (first 4000 chars),
`JCODE_HOOK_ERROR` (on failure).

### `session_start` / `session_end`

`session_start` fires when an agent session becomes active, with
`JCODE_HOOK_SOURCE` = `create` (brand new), `attach` (existing session object
attached), or `resume` (restored by id). `session_end` fires on normal close
(`JCODE_HOOK_SOURCE=close`).

### `post_tool`

Fires after every tool call. Extra fields: `JCODE_HOOK_TOOL_NAME`,
`JCODE_HOOK_STATUS`, `JCODE_HOOK_DURATION_MS`, `JCODE_HOOK_OUTPUT_BYTES` (on
success), `JCODE_HOOK_ERROR` (on failure).

## Gate hook: `pre_tool`

`pre_tool` runs **synchronously before every tool call** and can block it:

- The hook receives `JCODE_HOOK_TOOL_NAME` plus the full tool input JSON on
  **stdin** (and a 16 KB-truncated copy in `JCODE_HOOK_TOOL_INPUT`).
- **Exit 0**: allow the call.
- **Exit 2**: block the call. The hook's stderr (trimmed, capped at 2000
  chars) is returned to the model as the tool error, so the model can adapt.
- **Anything else fails open** with a logged warning: other exit codes,
  timeout (`pre_tool_timeout_ms`, default 5s), missing binary, spawn errors.

Fail-open is deliberate: a broken policy script should degrade to "no policy"
rather than brick every session. If you need fail-closed semantics, make the
hook itself robust (it is your trust boundary, not jcode).

## Tool-input transformers: `pre_tool_transform`

Transformers are synchronous external plugins that can replace a tool's JSON
input before it is validated and executed. Configure one or more in order:

```toml
[hooks]
pre_tool_transform = ["~/.jcode/plugins/rtk-transform"]
pre_tool_transform_timeout_ms = 500
```

Each transformer receives the current complete tool-input JSON on stdin and
`JCODE_HOOK_TOOL_NAME`, `JCODE_HOOK_SESSION_ID`, and `JCODE_HOOK_CWD` in its
environment. It may print a replacement JSON object to stdout. Empty stdout,
invalid JSON, a non-zero exit, a spawn error, or a timeout leaves the input
unchanged. This fail-open contract makes optional integrations safe to enable.

Transformers compose in configuration order and run before the `pre_tool`
policy gate, so policies evaluate the final transformed input. Keep plugins
small and tool-specific. For example, an RTK plugin should only change
`bash.command` when `rtk rewrite` returns a replacement.

### Example policy script

```bash
#!/usr/bin/env bash
# ~/bin/jcode-tool-policy
# stdin: tool input JSON. Env: JCODE_HOOK_TOOL_NAME, JCODE_HOOK_SESSION_ID...
input=$(cat)

case "$JCODE_HOOK_TOOL_NAME" in
  bash)
    if grep -qE 'rm -rf /([^a-zA-Z]|$)|mkfs|dd if=' <<<"$input"; then
      echo "blocked: destructive shell command" >&2
      exit 2
    fi
    ;;
  write|edit)
    if grep -q '"file_path":"/etc/' <<<"$input"; then
      echo "blocked: writes to /etc are not allowed" >&2
      exit 2
    fi
    ;;
esac
exit 0
```

## Example: tmux status + desktop notification on turn end

```bash
#!/usr/bin/env bash
# ~/bin/jcode-turn-notify
if [ "$JCODE_HOOK_STATUS" = ok ]; then icon=✅; else icon=❌; fi
tmux display-message "jcode $icon ${JCODE_HOOK_SESSION_ID:0:12}" 2>/dev/null
notify-send "jcode turn $JCODE_HOOK_STATUS" \
  "${JCODE_HOOK_LAST_ASSISTANT_TEXT:0:120}" 2>/dev/null
exit 0
```

## Example: JSON event log of all hook activity

Point several hooks at one script and fan out on `JCODE_HOOK_EVENT`:

```bash
#!/usr/bin/env bash
# ~/bin/jcode-event-log
echo "$JCODE_HOOK_PAYLOAD" >> ~/.local/state/jcode-events.jsonl
```

```toml
[hooks]
turn_end      = "~/bin/jcode-event-log"
session_start = "~/bin/jcode-event-log"
session_end   = "~/bin/jcode-event-log"
post_tool     = "~/bin/jcode-event-log"
```

## Design notes

- Hook lookups are config-driven and re-read on config reload; you can add or
  change hooks without restarting jcode.
- Hot paths (`pre_tool`/`post_tool`) check whether a hook is configured before
  building any payload, so unconfigured hooks cost ~nothing.
- The recursion guard (`JCODE_HOOKS_DISABLED=1`) means a hook may safely call
  `jcode` CLI commands without re-triggering hooks in that nested process.
