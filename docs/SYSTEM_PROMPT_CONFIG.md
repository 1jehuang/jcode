# Configuring the System Prompt

jcode builds its system prompt from several layers. Two of them are user-editable
files, so you can tune agent behavior without rebuilding.

## Layers (in order)

1. **Base system prompt** — built-in `crates/jcode-base/src/prompt/system_prompt.md`,
   overridable by file (see below).
2. Capability modules (e.g. Mermaid guidance).
3. Self-dev guidance (self-dev sessions only).
4. `AGENTS.md` — project `./AGENTS.md` and global `~/AGENTS.md`.
5. Prompt overlay — `./.jcode/prompt-overlay.md` and `~/.jcode/prompt-overlay.md`.
6. Preferred tools — `./.jcode/preferred-tools.md` and `~/.jcode/preferred-tools.md`.
7. Memory and the active skill prompt (dynamic, not cached).

## Adding guidance (most common)

Append instructions without touching the default prompt:

- `~/.jcode/prompt-overlay.md` — applies everywhere.
- `./.jcode/prompt-overlay.md` — applies to one project.

Both are included when present.

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
