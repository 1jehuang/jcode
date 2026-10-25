# Session agent instructions

Agent profiles are named Markdown instructions selected for one Jcode session. They do not change the model, tool permissions, or other sessions.

## Create a profile

Put a plain Markdown file in either location:

- `~/.jcode/agents/<name>.md` for a reusable personal profile.
- `<project>/.jcode/agents/<name>.md` for a project profile.

When `JCODE_HOME` is set, it replaces `~/.jcode`. A project file with the same name takes precedence. Names start with an ASCII letter or digit and contain only ASCII letters, digits, `_`, or `-`. The names `off` and `reload` are reserved.

For example, create `.jcode/agents/reviewer.md`:

```markdown
# Review approach

Focus on correctness, regressions, security, and missing tests.
Explain concrete findings with file locations before suggesting broad refactors.
Preserve unrelated changes.
```

Use nonempty plain Markdown. YAML frontmatter is rejected, rather than interpreted as model settings or permissions.

## Select a profile

Open `/agents`, choose **Instruction profile [session]**, then choose a profile. **None** leaves the session without an extra profile. Opening the picker or moving its highlight does not apply anything; confirm a row to change this session.

Profiles are never selected automatically. A new session starts with None unless you explicitly pass `--agent`. Resuming a session keeps the profile you previously applied there.

The text commands remain available:

```text
/agent
/agent reviewer
/agent reload
/agent off
```

`/agent` shows the current selection and available profiles. Selecting a name saves its resolved instructions in that session. `reload` explicitly reads the selected file again; `off` removes the selection. Change the profile between turns. A request during a running turn is rejected without interrupting that turn.

For a new CLI session:

```sh
jcode --agent reviewer
```

`--agent` is for a new interactive session. It cannot be combined with `--resume` or a subcommand such as `run`. To change an existing session, use `/agent` inside it.

Selection is resolved on the server using the session's project directory, including SSH sessions. It is not a change to `AGENTS.md` or a global default. `/agents` also retains its role/model configuration entries. Their global scope controls never make an instruction profile global; profiles always apply only to the current session.

### If the picker cannot load profiles

Both the TUI client and its connected server must support instruction profiles. Updating the client binary alone does not replace an already-running server. An older server can reject the profile-list request even though the new client shows the picker entry.

The picker keeps the failure visible without changing any instructions. If the server is busy, finish the current turn and reopen `/agents`. If it reports an unsupported command or incompatible response, upgrade the connected server too. For SSH sessions, that means the remote server.

Reloading a shared server can interrupt running turns and background work across sessions. Wait for that work to finish before using `jcode server reload`; do not reload it merely to dismiss a picker error.

## Persistence and composition

The session stores a snapshot of the profile name and instructions. Resuming retains that snapshot even if the source file changes or disappears. Forks inherit the snapshot; unrelated sessions do not. Use `/agent reload` when you deliberately want updated file contents.

`/clear` starts a new session and resets the profile along with the other session-specific instructions.

The profile is additive to normal project instructions, skills, and the normal system prompt. The existing API's full `system_prompt` replacement remains a separate feature; a selected profile is appended to that replacement too. Selecting a profile never rewrites global or project instruction files.
