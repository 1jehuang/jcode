# `/resume` behavior

`/resume`, `/session`, and `/sessions` open the interactive session picker. They are local UI commands and must not be sent as chat prompts.

## Default action

- `Enter` resumes the highlighted session in the current terminal.
- `Ctrl+Enter` opens the highlighted session in a new terminal.
- `Esc`, `q`, or `Ctrl+C` closes the picker without changing sessions.

The default is controlled by:

```toml
[keybindings]
session_picker_enter = "current-terminal"
```

Set `session_picker_enter = "new-terminal"` to swap `Enter` and `Ctrl+Enter`.

## Current-terminal resume

When resuming in the current terminal:

- Jcode sessions switch the current workspace/client to that session.
- Importable external sessions are first converted to a Jcode session, then resumed in place.
- If multiple sessions are selected, only the first selected target is resumed in place and the UI tells the user this.
- The picker closes after queueing the in-place resume.

## New-terminal resume

When opening in a new terminal:

- Each selected target opens in its own terminal when possible.
- If terminal launch is unavailable, the UI prints manual `jcode --resume <id>` commands.
- The picker remains open after launching and clears the current multi-selection so more sessions can be opened.

## Saved sessions

`/save [label]` bookmarks the current session so it appears in the saved section of the picker. `/unsave` removes that bookmark.
