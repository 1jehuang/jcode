# OpenAI Apps SDK, MCP, Apps UI, and Assistant Review

Status: review snapshot
Updated: 2026-04-28

This page records the current repo status after reviewing recent commits and branches for OpenAI Apps SDK, MCP server/client, apps UI, and assistant-related changes.

## Scope reviewed

Recent history reviewed from `windows-port`, `origin/master`, `fork/master`, and the desktop/UI branches, including the 2026-04-27 to 2026-04-28 merge window.

Notable commits in scope:

- `e5a766d5` - merge `master` into `windows-port`
- `78cba3ff` - lazy OpenAI provider hot-init on `set_model` / `complete`
- `d88beb19`, `2d60bca6`, `810973df`, `944be7ba` - desktop header/status/composer/markdown polish
- `894b6677` - desktop assistant markdown rendering
- `eefff4c5` - named OpenAI-compatible provider profiles
- `e8b7dd67`, `c60ecb61` - OpenAI ChatGPT image generation output/support
- `8f762f50`, `c5a16915`, `633045f3` - MCP failure logging, Windows test stabilization, skill registry reload visibility

## Executive summary

- jcode currently implements an MCP client, not an OpenAI Apps SDK server.
- No tracked branch currently contains OpenAI Apps SDK descriptors such as `openai/outputTemplate`, widget resources, Apps SDK `_meta` fields, or component-resource registration.
- MCP support is implemented for stdio JSON-RPC tool discovery/calls and is surfaced through the normal jcode tool registry.
- The desktop/apps UI work is active and now includes a functional single-session desktop surface, assistant markdown rendering, transcript/tool cards, image attachments, model picker, session switcher, stdin response flow, selection/copy, and status labels.
- Assistant vision support has improved: tool outputs can carry images, `read` sends supported image files to the model when under the size limit, browser screenshots can be attached as image outputs, and OpenAI ChatGPT mode can use native image generation.

## OpenAI Apps SDK status

No OpenAI Apps SDK integration is present in the reviewed code.

Searched for Apps SDK indicators:

- `Apps SDK`
- `OpenAI Apps`
- `apps UI`
- `openai/outputTemplate`
- `widgetDomain`
- component resource registration
- tool descriptor `_meta` fields used by the Apps SDK

Result: no implementation was found. The closest related systems are:

- jcode's MCP client, which consumes MCP tools from external stdio servers.
- jcode's normal tool schema generation for LLM providers.
- the desktop/TUI presentation layers that render assistant/tool output.

If OpenAI Apps SDK support is added later, it should be documented as a separate layer from the current MCP client. It would likely need:

1. A server/export surface that exposes Apps SDK-compatible tool descriptors.
2. Tool descriptor `_meta` fields such as output templates and invocation statuses.
3. Component/resource registration for any iframe/widget UI.
4. A clear boundary between private tool result metadata and transcript-visible content.
5. Tests that validate Apps SDK descriptor/resource payloads independently from the existing provider tool schemas.

## MCP status

See also [`../PLAN_MCP_SKILLS.md`](../PLAN_MCP_SKILLS.md).

Current MCP implementation:

- Config files: `~/.jcode/mcp.json`, `.jcode/mcp.json`, and compatibility import/read paths for `.claude/mcp.json` plus Codex `~/.codex/config.toml`.
- Runtime management: built-in `mcp` tool with `list`, `connect`, `disconnect`, and `reload`.
- Tool naming: discovered server tools are registered as `mcp__<server>__<tool>`.
- Startup behavior: configured servers connect in the background and send `McpStatus` updates to clients.
- Shared pool: daemon mode reuses `shared: true` MCP server processes across sessions and keeps `shared: false` servers per-session.
- Failure visibility: connection and reload failures are logged and returned through tool output.

Important files:

- `src/mcp/protocol.rs`
- `src/mcp/client.rs`
- `src/mcp/manager.rs`
- `src/mcp/pool.rs`
- `src/mcp/tool.rs`
- `src/tool/mcp.rs`
- `src/tool/mod.rs`
- `src/server.rs`

## Apps UI / desktop UI status

The desktop app is no longer only a concept doc. The current prototype has real single-session functionality and custom WGPU-rendered presentation.

Implemented or recently polished pieces include:

- single-session desktop mode on the shared jcode server
- model picker overlay and model cycle shortcut
- session switcher
- prompt navigation and copy
- character-precise transcript selection/copy
- TUI-style input editing and numbered composer footer
- stdin response flow for interactive tools
- clipboard/workspace image attachments
- transcript roles and tool transcript cards
- assistant markdown rendering through `pulldown-cmark`
- card styling for code blocks, quote blocks, tables, tools, and errors
- status labels, spinner/prompt rendering, build version in the header

Important files:

- `crates/jcode-desktop/src/main.rs`
- `crates/jcode-desktop/src/single_session.rs`
- `crates/jcode-desktop/src/single_session_render.rs`
- `crates/jcode-desktop/src/session_launch.rs`
- `crates/jcode-desktop/src/workspace.rs`

## Assistant rendering and media status

Assistant-related updates span provider behavior, tool output, and UI rendering.

### Markdown rendering

Desktop assistant text is parsed with `pulldown-cmark` and mapped into styled lines. Supported presentation includes:

- headings
- block quotes
- ordered and unordered lists
- fenced code blocks
- tables
- links
- images as textual link placeholders
- emphasis/strong/strikethrough/task-list marker preservation

The desktop renderer then applies separate styles/colors/cards for assistant headings, code, quotes, tables, links, tools, metadata, status, and errors.

### Vision and generated images

Current assistant/image behavior:

- User-pasted images are represented as `ContentBlock::Image`.
- `ToolOutput` supports `images: Vec<ToolImage>`.
- Tool images are converted into `ContentBlock::Image` blocks after the corresponding tool result.
- `read` sends supported image files to the model for vision analysis when the file is at most 20 MB.
- Browser screenshots can attach labeled image output.
- OpenAI ChatGPT mode adds the native `image_generation` tool and saves generated images under `.jcode/generated-images/` with JSON metadata.

This means the older limitation where image files were only displayed in the terminal and not sent to the model is no longer accurate.

## Documentation implications

Updated in this pass:

- `PLAN_MCP_SKILLS.md` now reflects implemented MCP/dynamic registry support instead of the old pre-implementation plan.
- `docs/DESKTOP_SINGLE_SESSION_DESIGN.md` now reflects the active prototype and recent desktop implementation details.
- `README.md` now links this review page and clarifies runtime MCP management.

Remaining suggested docs work:

- Add a dedicated MCP server authoring/testing guide.
- Add a desktop user guide once `jcode-desktop` packaging and launch flows settle.
- If Apps SDK support is intentionally planned, add an RFC before implementation so the descriptor/resource boundary does not get conflated with the existing MCP client.
