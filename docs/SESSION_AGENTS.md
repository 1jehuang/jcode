# Session agent instructions

Agent profiles are named Markdown instructions selected for one Jcode session. They do not change the model, tool permissions, or other sessions.

## Create a profile

Put a Markdown file, optionally with YAML frontmatter, in either location:

- `~/.jcode/agents/<name>.md` for a reusable personal profile.
- `<project>/.jcode/agents/<name>.md` for a project profile.

When `JCODE_HOME` is set, it replaces `~/.jcode`. A project file with the same name takes precedence. Names start with an ASCII letter or digit and contain only ASCII letters, digits, `_`, `-`, or internal single dots, such as `kimik3.1`. Trailing dots, consecutive dots, and path separators are rejected. The names `off` and `reload` are reserved.

For example, create `.jcode/agents/reviewer.md`:

```markdown
# Review approach

Focus on correctness, regressions, security, and missing tests.
Explain concrete findings with file locations before suggesting broad refactors.
Preserve unrelated changes.
```

Plain Markdown and OpenCode-style YAML frontmatter are supported. For example, the same `reviewer.md` can contain:

````markdown
---
description: Review correctness and regressions
mode: all
---
# Review approach

Focus on correctness, security, and missing tests.
````

The filename defines the profile name, even if the header contains `name`. YAML must be a valid mapping; an unclosed/malformed header or an empty instruction body is rejected. The exact Markdown body after the closing `---` (or `...`) is snapshotted, without the YAML header. Plain Markdown is kept unchanged. Standalone `.yaml`/`.yml` definitions are not discovered.

**This is instruction-file compatibility, not OpenCode runtime-policy compatibility.** Frontmatter metadata, including `mode`, `model`, `tools`, `permission`, and `permissions`, is not applied as Jcode settings. Your existing Jcode model, tools, and permissions remain unchanged. In particular, a frontmatter permission restriction does not enforce that restriction in Jcode; configure Jcode permissions separately. Selection reports this boundary explicitly.

## Select a profile

Open `/agents`, choose **Instruction profile [session]**, then choose a profile. Selecting a profile opens its **Instruction sources** options before applying it. A different profile starts with every optional source unchecked. Opening the current profile shows its saved choices.

Keep all extras unchecked to use the profile alone, or select individual sources to include in this session. **Apply** saves the profile and source choices together. **Cancel** or Escape leaves the session unchanged. Opening the picker, moving its highlight, or toggling a draft checkbox does not apply anything. A busy or failed Apply keeps the draft available to retry.

**None** turns the profile off and restores the session's ordinary instruction-loading rules. It does not disable ordinary project or global instructions.

Profiles are never selected automatically. A new session starts with None unless you explicitly pass `--agent`. Resuming a session keeps the profile you previously applied there.

The text commands remain available:

```text
/agent
/agent reviewer
/agent reload
/agent off
```

`/agent` shows the current selection, available profiles, and instruction-source information. Selecting a name saves its resolved instructions with no optional sources enabled. `reload` reads the selected profile body again and keeps the saved extra-source snapshots. Reapply the source options to refresh available selected extras from disk. `off` removes the profile and restores ordinary loading rules, including any stored full system-prompt override.

Change the profile between turns. A request during a running turn is rejected without interrupting that turn. Successful changes take effect on the next turn. Jcode resets cached tool definitions and the upstream conversation before sending that turn so a provider does not keep the previous Jcode-managed system instructions.

For a new CLI session:

```sh
jcode --agent reviewer
```

`--agent` is for a new interactive session. It cannot be combined with `--resume` or a subcommand such as `run`. To change an existing session, use `/agent` inside it.

Selection is resolved on the server using the session's project directory, including SSH sessions. It is not a change to `AGENTS.md` or a global default. `/agents` also retains its role/model configuration entries. Their global scope controls never make an instruction profile global; profiles always apply only to the current session.

### If the picker cannot load profiles

Both the TUI client and its connected server must support instruction profiles and source options. Updating the client binary alone does not replace an already-running server. An older server can reject discovery or lack source-option support even though the new client shows the picker entry. The client must not silently fall back to a selection that omits the requested source choices.

The picker keeps the failure visible without changing any instructions. If the server is busy, finish the current turn and reopen `/agents`. If it reports an unsupported command or incompatible response, upgrade the connected server too. For SSH sessions, that means the remote server.

Reloading a shared server can interrupt running turns and background work across sessions. Wait for that work to finish before using `jcode server reload`; do not reload it merely to dismiss a picker error.

## Which instructions are optional

While a profile is selected, Jcode excludes other Jcode-loaded instruction files unless you explicitly include them in that profile's source options. These are the known locations:

| Source | Project location | Global location |
| --- | --- | --- |
| Project and personal instructions | `<project>/AGENTS.md` | `~/AGENTS.md` |
| Base system-prompt replacement | `<project>/.jcode/system-prompt.md` | `~/.jcode/system-prompt.md` |
| Additional prompt guidance | `<project>/.jcode/prompt-overlay.md` | `~/.jcode/prompt-overlay.md` |
| Preferred-tool guidance | `<project>/.jcode/preferred-tools.md` | `~/.jcode/preferred-tools.md` |
| Swarm tool-description guidance | `<project>/.jcode/swarm-prompt.md` | `~/.jcode/swarm-prompt.md` |

A stored session `system_prompt` override is another optional source. It is preserved when you select a profile, but does not apply unless you include it. While profiled, that stored override is additive guidance. Turning the profile off restores its original full-replacement behavior.

Options identify individual files, not entire categories or directories. Including a project file does not enable its global counterpart or a new file that appears later. Jcode resolves the choices on the server. The client does not supply arbitrary file paths or instruction bodies. Canonical file identity prevents the same file from being offered twice through symlink aliases.

For ordinary Jcode prompts, selecting a `system-prompt.md` source retains that source's base-replacement semantics. If both project and global replacements are selected, the project source takes precedence. The swarm-prompt source also uses project-before-global replacement precedence. Selected `AGENTS.md`, prompt-overlay, and preferred-tools sources are combined in project-before-global order.

These paths are not a recursive Markdown search. Jcode's own loader does not automatically include ancestor `AGENTS.md`, `CLAUDE.md`, every file under `.jcode`, or arbitrary documentation. With `JCODE_HOME` set, global `.jcode` files come from that directory and the global `AGENTS.md` location becomes `$JCODE_HOME/external/AGENTS.md`.

### What stays available

The profile options control optional instruction sources, not the tool protocol or provider's system requirements. The built-in base prompt remains the default unless you explicitly include a supported replacement source. Applicable runtime context, self-development guidance, and recovery reminders remain.

The available-skills list is an index of skill names and descriptions, not every skill's full Markdown body. Explicit skill invocations, skill loading, and reference-file reads still work. Their contents enter context when used. Memory retrieval and the conversation history are separate from these file-source choices.

Providers that manage their own instructions, such as the native Claude Code runtime, can still load their own `CLAUDE.md` and other native context. The options control only what Jcode adds. They do not disable that provider's file discovery, managed settings, or system requirements. Selected Jcode instructions are forwarded to those providers as additional guidance rather than replacing their native system prompt.

## Persistence and composition

The session stores the selected profile body and each included source's contents as snapshots. Resuming retains those snapshots even if the original files change or disappear. Forks inherit them; unrelated sessions do not. Older saved profiles without explicit source choices use an empty optional-source list.

`/agent reload` refreshes only the profile body. Reapplying source options refreshes available selected extras. An included source that has become unavailable can retain its saved contents or be removed from the selection. A working-directory change does not automatically include files from the new directory.

`/clear` starts a new session and resets the profile and optional-source choices along with the other session-specific instructions. Selecting a profile never rewrites global or project instruction files.

### Context and token use

Excluded sources no longer contribute their bodies to new Jcode-managed system prompts or the swarm tool description. The profile itself, retained base and tool instructions, skill metadata, and conversation still use context. A cached prompt prefix still occupies the context window; caching does not make its text free of context cost.

Jcode estimates text tokens from UTF-8 byte length divided by four. This is a heuristic, not a provider tokenizer count. Compare the complete effective prompt and tool definitions when measuring savings, especially when an excluded user base prompt is replaced by the built-in default.

Changing source options does not erase previous messages, explicit file reads, tool results, memory, or summaries. An upstream conversation reset sends the current Jcode-managed instructions again with retained history. It is not a clean-history or secret-redaction feature.
