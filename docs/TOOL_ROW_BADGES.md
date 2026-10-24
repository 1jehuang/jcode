# Tool Row Badges: Duration and Timestamp

Completed tool transcript rows can carry two optional badges after the token
count:

```
✓ bash · Прогон acceptance-тестов · 223 tok · 2m 3s · 17:32:05
                             ^^^^^^^    ^^^^^    ^^^^^^^^
                             tokens     duration timestamp
```

Both are strictly opt-in and off by default, and each has its own config key,
so enabling one never turns on the other.

## How the duration badge works

`display.show_tool_duration` (default `false`) adds `· 2m 3s` to a tool row:
how long the call took.

- The agent loop measures each tool execution server-side and stores it in
  `StoredMessage.tool_duration_ms` when the tool result is recorded (#1453).
- Live rows get the duration the moment the `ToolDone` event lands; reloaded
  transcripts get it from the stored session, so both show the same badge.
- Compact buckets: `45ms` (under a second; a bare `0.0s` is noise), `42.3s`,
  `2m 3s`, `1h 05m`.
- Severity coloring mirrors the token-badge grading via a shared
  `severity_badge_color` helper: neutral < 10s, amber >= 10s, red >= 60s
  (`tool_duration_severity` in `jcode-core`).

## How the timestamp stamp works

`display.show_tool_timestamp` (default `false`) adds `· 17:32:05` to a tool
row: the wall-clock time when the call completed (#1454).

- Every stored message already records a wall-clock `StoredMessage.timestamp`;
  the stamp renders it through `RenderedMessage` -> `DisplayMessage` -> the
  tool row renderer, for both live transcripts after a history reload and
  session-picker previews.
- `display.timestamp_tz` (default `local`) picks the timezone for the stamp:
  - unset / `local` / `system`: the machine's local timezone;
  - `UTC+3`, `utc-5`, `UTC+0`, `UTC+05:30`, or a bare `3`: a fixed offset,
    useful for remote/devcontainer setups where the machine TZ differs from
    the user's;
  - anything else (a typo like `Moscow`): falls back to local, so rendering
    never breaks.
- The clock span is always the neutral blue-grey used by calm badges
  (`rgb(120,130,145)`). It never inherits the duration severity color, so a
  slow call can never make the timestamp read as an error state.

## Narrow terminals

Both badges ride with the preserved right-side suffix of the tool row. When
the transcript is narrow, the summary truncates first; token count, duration,
and timestamp stay visible (checked at 40/56/72/120 columns).

## Config

```toml
[display]
show_tool_duration = false   # · 2m 3s   (how long the call took)
show_tool_timestamp = false  # · 17:32:05 (when the call ran)
timestamp_tz = "local"       # or "UTC+3", "utc-5", "UTC+05:30", "3"
```
