# Desktop panels

The model-facing `panel` tool opens and manages separate desktop panels within the
current session. Each spawn creates a fresh panel, even for the same source file.
Panels persist across reconnects. Closing a linked panel never deletes its source.
The TUI uses the same state and displays a human-readable Markdown fallback for PDFs.

## Tool contract

- `action`: `spawn` (default), `update`, `focus`, `close`, or `list`.
- `spawn`: exactly one of `content` (Markdown) or `file_path` (Markdown/PDF).
  Optional `title` and `focus` (default `true`). Do not supply `panel_id`.
- `update`: existing same-session `panel_id`, exactly one of `content` or
  `file_path`, optional `title` and `focus` (default `false`). Unknown IDs fail
  rather than spawning. Title-only updates are not supported.
- `focus` / `close`: existing same-session `panel_id` only.
- `list`: no additional arguments.
- Standard `intent` is available for all actions.

Examples:

```json
{"content":"# Results\n\nAnalysis complete.","title":"Results"}
{"action":"spawn","file_path":"reports/analysis.PDF"}
{"action":"update","panel_id":"panel-<uuid>","content":"# Updated results"}
{"action":"focus","panel_id":"panel-<uuid>"}
{"action":"close","panel_id":"panel-<uuid>"}
{"action":"list"}
```

Mutation output identifies `panel_id` and `identity: side-panel://<session>/<id>`
in readable text. Tool metadata is a `SidePanelSnapshot`, including `pages[].id`
and `focused_page_id`, with `pdf_data` omitted to keep binary data out of tool
history. Full bytes travel only in panel state events. List and close return the
remaining current snapshot metadata.
Every mutation uses the existing `SidePanelUpdated` bus event and server
`side_panel_state` event, including tools invoked through the debug path.

## Files and protocol

Supported extensions are `.md`, `.markdown`, `.mdown`, `.mkd`, `.mkdn`, and `.pdf`,
case-insensitively. Relative paths resolve from the tool working directory.
Files are linked, so hydration/reload reads their current bytes.
PDF validation requires a `%PDF-` signature, not a complete semantic PDF parse.

PDF pages have `format: "pdf"` and optional `pdf_data` containing base64 bytes.
`content` always remains human-readable Markdown, never base64. Missing,
unreadable, malformed or oversized files clear PDF data and display an error
fallback on hydration/refresh instead of retaining stale content.

Native connections must opt in with `supports_pdf_panels: true` on `Subscribe`
to receive PDF format and payloads. The API bridge opts in. Other native clients,
including older TUIs, receive Markdown projections without `pdf_data`, preserving
their existing panel decoding and fallback display.

PDF limits are **20 MiB per file** and **32 MiB aggregate decoded PDF bytes per
session**. Spawn/update rejects over-budget files before modifying panel state.
Linked files that grow beyond the aggregate budget hydrate with an error fallback.
The bridge frame cap is 64 MiB, allowing base64 expansion and ordinary metadata.
These PDF limits do not bound arbitrarily large Markdown content.

`focus_revision` is a serde-defaulted monotonic integer in persisted state and
snapshots. Explicit focus requests increment it even when the same panel was
already focused. Background updates, list and hydration preserve it. Desktop
clients can distinguish fresh focus intent from replayed state without stealing
focus on background updates. Legacy snapshots deserialize with revision zero.

The legacy `side_panel` tool remains registered for compatibility, including
status/write/append/load/focus/delete. Load accepts Markdown/PDF. Write and append
remain Markdown-only, and appending to an existing PDF is rejected. Rust callers
can use `load_file` for either format or the preserved `load_markdown_file` wrapper.
