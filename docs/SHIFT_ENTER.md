# Shift+Enter and multi-line input

## The problem

Terminals send one byte for Enter: `0x0d`. The VT100-era encoding has nowhere to
record that Shift was held, so `Enter`, `Shift+Enter`, and `Ctrl+Enter` all
arrive as the same byte. An application cannot tell them apart, no matter how it
is written.

## How jcode handles it

The modern fix is the **kitty keyboard protocol**. The app asks the terminal to
disambiguate, and the terminal then sends `ESC[13;2u` for Shift+Enter
(keycode 13, modifier 2 = 1 + the shift bit).

jcode requests the protocol at startup (`enable_keyboard_enhancement`) and
crossterm decodes the result, so **on a capable terminal Shift+Enter works with
no setup**: kitty, Ghostty, WezTerm, Alacritty, foot, iTerm2 3.5+, Warp, and
VS Code 1.109+.

Three situations still break, and jcode handles each explicitly:

| Situation | Fix | Where |
| --- | --- | --- |
| Terminal ignores the request (Terminal.app) | Switch terminals, or map Shift+Return to `\033[13;2u` by hand | `/terminal-setup` explains |
| tmux does not forward extended keys | Write `extended-keys` settings to `~/.tmux.conf` | `/terminal-setup` applies |
| WezTerm needs an opt-in flag | Set `enable_kitty_keyboard = true` | `/terminal-setup` applies |

## `/terminal-setup`

Run it when Shift+Enter submits instead of inserting a newline. It queries the
terminal for real support rather than assuming, then either confirms the chord
already works, applies the needed configuration, or explains why configuration
cannot help.

The query matters: writing the activation escape sequence almost always
"succeeds" even on terminals that ignore it, so
`supports_modified_enter_reporting` asks the terminal directly (`CSI ? u`
followed by `CSI c`).

## Fallbacks

These work on every terminal because they do not depend on modifier reporting:

- **Trailing backslash then Enter** inserts a newline, matching shell line
  continuation. The first time you use it, jcode points you at
  `/terminal-setup`.
- **Option/Alt+Enter** works wherever the terminal sends `ESC` + `CR`, which
  includes Terminal.app with "Use Option as Meta Key" enabled.

## Why not just tell users to use the fallback?

Because Shift+Enter is what people expect, and on most terminals it is already
achievable. A fallback is a safety net, not a substitute for the chord working.

## Tests

- `tui::app::tests::shift_enter_csi_u_sequence_decodes_to_enter_plus_shift`
  feeds the exact bytes through a real PTY and asserts crossterm decodes
  Enter+SHIFT. This pins the sequence written into terminal configs to the
  sequence the app actually understands.
- `tui::app::tests::bare_carriage_return_decodes_without_shift` pins the
  underlying problem so the reason setup exists stays documented in code.
- `tui::terminal_setup::tests::*` cover config generation, idempotency, and not
  clobbering user config.
