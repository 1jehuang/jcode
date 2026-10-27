# Skills and MCP: how Claude Code does it, and what jcode does

Part 1 is the reference model, taken from what a live Claude Code session
actually receives (system prompt, reminders, tool contracts), not from guesses.
Part 2 maps each behavior to jcode. Part 3 lists what was built to close gaps.

## Part 1: Claude Code reference model

### Skills

| # | Behavior | What the model sees / what happens |
|---|----------|-------------------------------------|
| CS1 | Sources | `~/.claude/skills/<name>/SKILL.md`, `<project>/.claude/skills/...`, plugin skills (named `plugin:skill`), directory-scoped skills (named `apps/web:deploy`), legacy `.claude/commands/*.md`. |
| CS2 | Listing | One block: `The following skills are available for use with the Skill tool:` then `- name: description` per skill. Only name + description; the body is not loaded. Delivered as a reminder message in the conversation, so the cached system prompt never changes when skills change. |
| CS3 | Model invocation | A `Skill` tool, input `{skill, args?}`. Its description says to call it *first* when a listed skill covers the task, to use exact names from the listing, and not to call it again if a `<command-name>` block is already present this turn. The result injects the SKILL.md body into the turn. Some skills run in a subagent (`context: fork`) and return only their result. |
| CS4 | User invocation | `/name args` is expanded by the harness into the conversation (`<command-name>`, `<command-args>`, then the body) as a user-turn message, not a system-prompt edit. |
| CS5 | Frontmatter | `name`, `description`, `allowed-tools`, `argument-hint`, `disable-model-invocation` (hide from model listing), `user-invocable` (hide from `/` menu), `model`, `context: fork`. Body supports `$ARGUMENTS`. |
| CS6 | Supporting files | The skill's base directory is given with the body so relative files (`scripts/`, `reference.md`) can be read on demand. |

### MCP

| # | Behavior | What the model sees / what happens |
|---|----------|-------------------------------------|
| CM1 | Config | `~/.claude.json` (`mcpServers`, plus `projects[<path>].mcpServers`), project `.mcp.json`, plugin MCP, `--mcp-config`. `${VAR}` / `${VAR:-default}` expansion. stdio, http, sse, OAuth. |
| CM2 | Naming | Every tool is first-class: `mcp__<server>__<tool>`. |
| CM3 | Deferred loading | When tool definitions are large, MCP tools are deferred. The model gets **names only**: `The following deferred tools are now available via ToolSearch. Their schemas are NOT loaded — calling them directly will fail ... Use ToolSearch with query "select:<name>[,<name>...]"`. |
| CM4 | Tool search | `ToolSearch{query, max_results}`. Query forms: `select:A,B` (exact fetch), keywords, `+term rest` (term required in name). Result is a `<functions>` block with full JSON schemas; those tools are then callable directly. |
| CM5 | Late tools | When a server finishes connecting mid-session: name-only reminder for deferred tools, or full definitions if the tool is to be callable immediately. |
| CM6 | Server status | Reminders list servers that **need auth**, **failed to connect** (with the error, marked as untrusted data), and are **still connecting**. ToolSearch waits for connecting servers. |
| CM7 | Server instructions | `instructions` from each server's `initialize` result go into a `# MCP Server Instructions` section. |
| CM8 | Management | `/mcp` lists servers with status, reconnect, auth. |

The key design idea in both halves: **the model is told what exists (names +
one line), and pays for the full text (skill body, tool schema) only when it
asks for it.** Nothing in the cached system prompt moves when skills or
servers change.

## Part 2: jcode status (before this change)

Evidence paths are relative to `crates/`.

### Skills

| CC | jcode before | Gap |
|----|--------------|-----|
| CS1 sources | `~/.claude/skills` copied **once** into `~/.jcode/skills` on first run (`jcode-base/src/skill.rs` `import_from_external`); project `.jcode`/`.agents`/`.claude` skills; plugin skills un-namespaced; no `.claude/commands`. | New Claude Code skills never appeared. |
| CS2 listing | `# Available Skills` in the cached system prompt, frozen per session; late installs announced in the transcript (`agent/turn_execution.rs` `announce_late_skills`). | Text only said the *user* can run `/name`; nothing told the model to load skills itself. |
| CS3 model invocation | `skill_manage` (renamed `Skill` on Anthropic OAuth) with `load`; `args` accepted and ignored. | No `$ARGUMENTS`, vague tool description. |
| CS4 user invocation | `/name` set `active_skill`, rendered as `# Active Skill` in the **system prompt every turn** (`agent/prompting.rs`). | Cache miss on activation, body re-sent each turn, no base dir, no args. |
| CS5 frontmatter | `name` (required), `description`, `allowed-tools` (display only). | Skills without `name:` silently skipped; a `---` inside the YAML broke parsing. |
| CS6 supporting files | Base dir only in `skill_manage load`. | Missing on `/name`. |

### MCP

| CC | jcode before | Gap |
|----|--------------|-----|
| CM1 config | Reads `~/.jcode/mcp.json`, `~/.claude.json` (user + `projects[path]`), `~/.claude/mcp.json`, project `.jcode/mcp.json`, `.mcp.json`, `.claude/mcp.json`; `${VAR:-default}`; stdio/http/sse/OAuth (`jcode-base/src/mcp/protocol.rs`). | Close. Project `.mcp.json` starts **without approval**; precedence differs. |
| CM2 naming | `mcp__server__tool` (non-alnum → `_`). | Close. |
| CM3 deferred | Native `defer_loading` (Anthropic API, OpenAI gpt-5.4+) or `mcp_search` + `mcp_call` proxy. Proxy path announced late tools **with full input schemas** (cap 32), native path announced **nothing**. | Context blowup (a ~150-tool server cost tens of thousands of tokens), and on the native path the model could not see what exists. |
| CM4 search | `mcp_search{server?, query?}` keyword AND-match. | No exact `select:` form. |
| CM6 status | Failures only in the log. | Model not told about failed / needs-auth servers. |
| CM7 instructions | `initialize.instructions` not even parsed. | Missing. |
| results | Unknown content block (`resource_link`, `audio`) **failed the whole call**; `tools/list` pagination ignored. | Robustness. |

## Part 3: what was built

Skills (`jcode-base/src/skill.rs`, `jcode-base/src/prompt.rs`, `jcode-app-core/src/agent/*`, `jcode-app-core/src/tool/skill.rs`):

1. `~/.claude/skills` is read live on every (re)load, between `~/.jcode/skills` and `~/.agents/skills`; it shadows stale copies from the old first-run import, which no longer copies Claude skills.
2. Frontmatter: `name` optional (defaults to the folder name), `disable-model-invocation` (hidden from the model list, `skill_manage load` refuses it), closing fence must be its own line.
3. `/name args` now puts the skill into the **transcript once** (`inject_active_skill`), as `Base directory for this skill: <dir>` + body with `$ARGUMENTS` / `${CLAUDE_SKILL_DIR}` filled in. The system prompt no longer changes when a skill is activated. Re-injected only when the active skill changes or compaction dropped it.
4. `skill_manage load` uses the same text, honours `args` (falls back to `ARGUMENTS: ...` like Claude Code), and its description plus the Available Skills section now tell the model to load a matching skill first.

MCP (`jcode-base/src/mcp/*`, `jcode-app-core/src/tool/mcp.rs`, `jcode-app-core/src/agent/turn_execution.rs`):

5. Tool announcements are **names only** (cap 200), on both deferred paths, telling the model to load schemas with `mcp_search` `select:<name>[,<name>...]`, then call directly (native) or via `mcp_call` (proxy).
6. `mcp_search` accepts `select:a,b` for exact loading.
7. Server `instructions` are parsed and returned with that server's `mcp_search` results (cache-stable alternative to Claude Code's system-prompt section).
8. Unknown result blocks render as placeholders instead of failing; `resource_link` is shown; `tools/list` follows `nextCursor`. Result rendering is shared between direct MCP tools and `mcp_call`.

The `/mcp` and `/skills` dialogs (`jcode-tui/src/tui/dialog.rs`, built in a
parallel session) were then checked side by side against Claude Code 2.1.302
(`claude-code-ui-reference.md`) and brought in line:

9. Project config servers (`.mcp.json`, `.jcode/mcp.json`, `.claude/mcp.json`)
   wait for approval with Claude Code's prompt and three answers (Esc = no),
   asked at session start and from `/mcp`. Answers live in
   `~/.jcode/mcp-approvals.json`, outside the repo: jcode has no folder-trust
   prompt, so a repository must not be able to pre-approve its own servers
   (deliberate difference: Claude Code stores them in the project's
   `.claude/settings.local.json`). Enable/disable on a project server is
   approve/reject, as in Claude Code.
10. The model is told, once per problem, which servers failed to connect (error
    marked as data, not instructions) or wait for approval.
11. SKILL.md frontmatter uses Claude Code's lenient fallback (quote loose
    values and retry), so skills like `codebase-memory` and `codex-build`
    with `Triggers on: …` descriptions load instead of being dropped.
12. Plugin skills: only `<plugin root>/skills` is loaded (Claude Code's
    `skillsPath`), named `plugin:skill`, `locked by plugin` in `/skills`;
    `enabledPlugins: false` is honoured; a bare `/skill` still resolves when
    unambiguous. Previously jcode also loaded plugin template trees (11 bogus
    skills from `setupdotclaude/template/skills`).
Review fixes after an independent pass: a repo's `"disabled": false` no longer
slips past the gate; connect-on-first-call, `ensure_server_connected` and the
model's `mcp connect` refuse unapproved/rejected project servers and servers
disabled in `/mcp` (one fresh, on-disk approval state feeds every path,
including `mcp_search`'s cached schemas); approve-all starts every waiting
server; a skill compacted out of view is injected again; the Claude Code
runtime keeps an invoked skill when seeding a new session; disabling all
plugins no longer falls back to scanning every cached plugin; reloading a
plugin skill keeps its prefix; `tools/list` paging stops on any repeated
cursor (max 100 pages); a missing `description` defaults to the body's first
line like Claude Code; the TUI's local mode uses the same model skill list;
the internal `/mcp` and status calls bypass user tool hooks and telemetry.

13. Dialog details: column-aligned rows and detail labels, numbered server
    actions with digit keys, tool `read-only` / `destructive` / `open-world`
    hints from MCP annotations, `No changes` on a no-op `/skills` close.

Not built:

- Authenticate from `/mcp` (jcode's OAuth login stays `jcode mcp login`).
- MCP prompts as `/server:prompt (MCP)` commands, resources, `list_changed`.
- `allowed-tools` enforcement, `$1`-style positional args, `` !`cmd` ``
  preprocessing, `context: fork`, legacy `.claude/commands/*.md`.
- Manifest-declared plugin `skillsPaths` and plugin-provided MCP servers.
- Walking parent directories for `.mcp.json` (Claude Code does; jcode reads
  the project root only, which is also the safer default).
