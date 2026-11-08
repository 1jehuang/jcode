# Automatic tool background timeout

Set the foreground budget in `~/.jcode/config.toml`:

```toml
[tools]
auto_background_timeout_ms = 10000
```

The default is **3000 milliseconds (3 seconds)**, preserving the existing behavior.
Values are nonnegative integer milliseconds. Set `auto_background_timeout_ms = 0`
to disable automatic handoff. Negative, fractional, and string values are invalid.
The normal configuration loader applies this setting (`$JCODE_HOME/config.toml`
when `JCODE_HOME` is set). Each tool call snapshots its budget when execution starts.

This budget applies to the agent's streaming TUI, nonstreaming turns, and SDK
callback execution paths. When it expires, Jcode adopts the still-running tool as
a background task, returns its task ID, and lets the agent continue. It does not
cancel or restart the operation. Handoff messages report the configured duration,
including subsecond values. Use `bg` with `action = "wait"` to collect completion.

`bg wait` itself is exempt so a dependency wait does not repeatedly move into the
background. Manual background requests and server-reload signals still work in
streaming turns, even when automatic handoff is disabled. Tools that finish before
the budget expires return their ordinary result directly.

This setting is not an execution deadline. It does not change a tool's own timeout,
explicit background options, or tool-specific foreground-to-background behavior
(such as bash and agentgrep). A tool may independently return a background task
before this agent-wide budget expires. In particular, setting this value to zero
does not disable those tool-specific behaviors.
