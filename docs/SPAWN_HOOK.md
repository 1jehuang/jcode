# Spawn Hook: External Control of Headed Session Spawns

jcode opens new terminal windows in several flows: swarm agent spawning
(`swarm spawn` with `spawn_mode=visible`), resume-in-new-terminal, self-dev
sessions, restart restores, and jade relay launches. By default jcode detects
an installed terminal emulator (kitty, wezterm, alacritty, gnome-terminal, ...)
and opens a new OS window.

The **spawn hook** lets an external program take over this spawn so it can
decide *where and how* the session appears: a tmux pane, a kitty tab, a zellij
pane, a tab in a wrapper app like herd, a specific monitor/workspace, etc.

## Configuration

```toml
# ~/.jcode/config.toml
[terminal]
spawn_hook = "tmux new-window"
```

Or per-environment:

```bash
export JCODE_SPAWN_HOOK="tmux new-window"
# An empty value disables a config-file hook:
export JCODE_SPAWN_HOOK=
```

Env always wins over the config file.

## Contract

When a headed spawn happens and a hook is configured, jcode runs:

```
<spawn_hook> <jcode-binary> <args...>
```

- The hook command is parsed shell-style (quotes and backslash escapes work),
  but it is executed directly, not through a shell.
- The jcode binary and its full argument list are appended as extra argv
  entries (the familiar `$TERMINAL -e <cmd>` convention).
- The hook's working directory is the session working directory.
- The hook process is detached; jcode does not wait for it.
- If the hook fails to start (binary missing, parse error), jcode logs a
  warning and falls back to its built-in terminal detection.

### Metadata environment

The hook (and any terminal spawned by the built-in fallback) receives:

| Variable | Meaning |
| --- | --- |
| `JCODE_SPAWN_KIND` | Why the spawn happened: `swarm-agent`, `resume`, `selfdev`, `restart`, `jade-relay` |
| `JCODE_SPAWN_SESSION_ID` | The jcode session the window will run |
| `JCODE_SPAWN_TITLE` | Suggested window/tab title (includes session icon + name) |
| `JCODE_SPAWN_CWD` | Session working directory |
| `JCODE_SPAWN_PROGRAM` | Path of the jcode binary to execute |
| `JCODE_SPAWN_COMMAND` | Full command line, shell-escaped, for hooks that take one shell string |
| `JCODE_SPAWN_SWARM_ID` | (swarm spawns) The swarm the agent joins |
| `JCODE_SPAWN_COORDINATOR_SESSION_ID` | (swarm spawns) The coordinator session that requested the spawn |
| `JCODE_FRESH_SPAWN` | `1` when the spawn is a fresh window handoff |

### Client terminal environment (multi-terminal routing)

The jcode server process is long-lived: it captures terminal-identifying env
vars (`ZELLIJ_SESSION_NAME`, `TMUX`, `DISPLAY`, `KITTY_WINDOW_ID`, ...) once at
startup. When you later open a *new* terminal/tmux/zellij session and connect a
client to the same server, the server's copies are stale, so a spawn hook run by
the server would otherwise target the *old* terminal.

To fix this (see issue #405), each connecting client snapshots its own
terminal-identifying env and sends it to the server. When a spawn hook runs, the
server re-exports the requesting client's values so the hook follows the
terminal the user is actually attached to:

- The native variable (e.g. `ZELLIJ_SESSION_NAME`) is overridden with the
  client's value, so hooks that read it directly target the right session.
- A `JCODE_CLIENT_<NAME>` alias (e.g. `JCODE_CLIENT_ZELLIJ_SESSION_NAME`,
  `JCODE_CLIENT_TMUX`, `JCODE_CLIENT_DISPLAY`) is also exported so a hook can
  explicitly distinguish the client's terminal from the server's.

Covered keys include the terminal multiplexers (zellij, tmux, screen), terminal
emulators (kitty, wezterm, ghostty, alacritty, iTerm, Windows Terminal,
handterm), and the display server (`DISPLAY`, `WAYLAND_DISPLAY`). Only vars that
the client actually has set are forwarded.

## Examples

When jcode detects that the requesting client is inside tmux, its built-in
launcher automatically opens headed spawns in a right-side pane targeted at the
requesting `TMUX_PANE`. This covers `/split`, `/fork`, resume-in-new-terminal,
self-dev, and visible agent spawns. `JCODE_TERMINAL` can explicitly choose a
terminal emulator instead, and a configured `spawn_hook` still takes complete
precedence.

### tmux: one window per agent

```toml
[terminal]
spawn_hook = "tmux new-window"
```

Use a hook when you want to override the automatic pane behavior. For example,
`tmux new-window <jcode> --resume ses_x` runs the command in a new window of
the current tmux server. To explicitly preserve the right-side pane behavior:

```toml
[terminal]
spawn_hook = "tmux split-window -h"
```

### kitty: one tab per agent (remote control)

```toml
[terminal]
spawn_hook = "kitty @ --to unix:/tmp/kitty.sock launch --type=tab --"
```

### Custom router script

For full control (placement, titles, swarm vs resume routing), point the hook
at a script:

```toml
[terminal]
spawn_hook = "~/bin/jcode-spawn-router"
```

```bash
#!/usr/bin/env bash
# ~/bin/jcode-spawn-router
# argv: the jcode command to run ("$@"). Env: JCODE_SPAWN_* metadata.

case "$JCODE_SPAWN_KIND" in
  swarm-agent)
    # Swarm workers as tmux panes in a window named after the swarm.
    tmux new-window -n "swarm:${JCODE_SPAWN_SWARM_ID:0:8}" "$@" 2>/dev/null \
      || tmux split-window "$@"
    ;;
  *)
    # Everything else as a normal terminal window.
    kitty --title "$JCODE_SPAWN_TITLE" -e "$@" &
    ;;
esac
```

A hook that exits non-zero after launching nothing will NOT trigger the
built-in fallback (jcode only falls back when the hook process cannot be
started), so a router script should handle its own fallback like the example
above.

### Single-shell-string consumers

Some launchers want one shell command string instead of argv. Use
`$JCODE_SPAWN_COMMAND`:

```bash
#!/usr/bin/env bash
zellij action new-pane -- bash -lc "$JCODE_SPAWN_COMMAND"
```

## Programmatic discovery

Programs that wrap jcode (e.g. herd-style session managers) can set
`JCODE_SPAWN_HOOK` in the environment of the `jcode` server process they
launch. Every headed spawn the server performs, including swarm agents
requested by coordinators over the socket protocol, will then route through
the wrapper's hook.

## Focus hook

When jcode wants to bring an existing session window to the foreground (e.g.
after launching a self-dev window), it normally does a best-effort
wmctrl/xdotool title search on X11. That doesn't work under Wayland or inside
multiplexers, and a wrapper that owns placement should also own focus:

```toml
[terminal]
spawn_hook = "tmux new-window"
focus_hook = "~/bin/jcode-focus"   # env: JCODE_FOCUS_SESSION_ID, JCODE_FOCUS_TITLE
```

```bash
#!/usr/bin/env bash
# ~/bin/jcode-focus
tmux select-window -t "$(tmux list-windows -F '#{window_id} #{window_name}' \
  | grep -F "$JCODE_FOCUS_TITLE" | head -1 | cut -d' ' -f1)"
```

Env override: `JCODE_FOCUS_HOOK` (empty value disables a config-file hook).
If the hook fails to start, jcode falls back to the built-in focus path.
