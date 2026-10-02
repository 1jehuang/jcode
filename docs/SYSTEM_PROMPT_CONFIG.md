# Configuring the System Prompt

jcode builds its system prompt from several layers. Two of them are user-editable
files, so you can tune agent behavior without rebuilding.

## Layers (in order)

1. **Base system prompt** — built-in `crates/jcode-base/src/prompt/system_prompt.md`,
   overridable by file (see below).
2. Capability modules (e.g. Mermaid guidance).
3. Product-specific self-dev guidance. Sessions rooted in a Jcode Desktop
   checkout automatically receive the Desktop prompt and `desktop_selfdev` tool,
   separate from CLI/TUI self-dev flags, `selfdev`, and `debug_socket`.
4. `AGENTS.md` — project `./AGENTS.md` and global `~/AGENTS.md`.
5. `PROGRESS.md` — project `./PROGRESS.md` only (never global). Cross-session
   STATE, not instructions: loads after the instruction layers. See
   "Cross-session progress" below.
6. Prompt overlay — `./.jcode/prompt-overlay.md` and `~/.jcode/prompt-overlay.md`.
7. Preferred tools — `./.jcode/preferred-tools.md` and `~/.jcode/preferred-tools.md`.
8. Memory and the active skill prompt (dynamic, not cached).

## Cross-session progress (`PROGRESS.md`)

Long-running work survives context resets through a progress file the agent
itself maintains:

- **Read**: jcode loads `./PROGRESS.md` into every new session's bootstrap
  snapshot automatically (when present; absent means zero behavior change).
- **Write**: the agent updates it at milestones and session end via the edit
  tool. Core never writes it — write triggers are judgment calls, and a wrong
  auto-write would corrupt the next session's bootstrap.
- **Shape** (resume-critical subset of a compaction summary): task (1-2
  sentences), current state, next steps priority-ordered, blocked items with
  unblock conditions, exact file paths and identifiers. A compaction summary
  can serve as the progress update verbatim.
- **Lifecycle**: start a session by reading `PROGRESS.md` plus `git log`;
  end mergeable-clean with the file updated. One file, one project — never
  global (progress belongs to exactly one project).
- **Keep it tight**: the whole file loads into every session's bootstrap, so
  prune ruthlessly — current state + next steps, not history. Move detail to
  the session transcript (it stays on disk); the progress file is a pointer
  to resume, not an archive.

## Adding guidance (most common)

Append instructions without touching the default prompt:

- `~/.jcode/prompt-overlay.md` — applies everywhere.
- `./.jcode/prompt-overlay.md` — applies to one project.

Both are included when present. For layers 4, 6–7, if the project and global paths
resolve to the same canonical path (for example, when working in `$HOME` or using
symlink aliases), the file is included once under its project heading. Distinct
files are still both included, even when their contents match. The global
`.jcode` directory respects `JCODE_HOME` when set.

## Replacing the base prompt

To fully replace layer 1, create either file:

- `./.jcode/system-prompt.md` (project, highest precedence)
- `~/.jcode/system-prompt.md` (global)

The first non-empty file wins; otherwise the built-in default is used. An empty or
whitespace-only file falls back to the default, so you cannot accidentally ship an
empty prompt.

This replaces only the base prompt. AGENTS.md, overlays, skills, and memory still apply.

## Notes

- Changes to these files take effect for **new sessions**; a running session keeps the
  prompt captured at start.
- Editing the built-in `system_prompt.md` requires a rebuild (`selfdev build-reload`),
  since it is embedded with `include_str!`.
- Swarm model-routing guidance has its own analogous file: `.jcode/swarm-prompt.md`.
  Use `/swarm-prompt` to edit the active project or global file. New agents load
  the latest contents immediately; already-running agents keep the prompt they
  captured at session creation so their tool definition and context cache stay stable.

## Direct SDK overrides

SDK callers can replace the **complete assembled system prompt** when creating a
session, without writing files:

```typescript
const session = await client.createSession({
  workingDir: process.cwd(),
  systemPrompt: "You are a concise programming tutor.",
});
```

Rust callers use `create_session_with_options(CreateSessionOptions {
working_dir: None, system_prompt: Some("You are a concise programming tutor.".into())
})`. The existing `create_session(working_dir)` API remains available.

Unlike `system-prompt.md`, which replaces only the base layer, this option replaces
all assembled prompt layers. Omit the option to retain normal Jcode prompting.
An empty string explicitly selects an empty system prompt. The override belongs
to that session and is persisted for resume and inherited by forks. Attaching to
an existing session does not change its prompt. This requires a daemon version
that supports the `system_prompt` session-creation field.
