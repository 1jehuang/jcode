# Skill arguments

Typing `/name some arguments` activates the skill `name` and substitutes the
text after the name into the skill body. The syntax follows Claude Code.

| Placeholder | Meaning |
|---|---|
| `$ARGUMENTS` | the whole argument string as typed |
| `$ARGUMENTS[N]`, `$N` | word `N`, counting from 0, shell-style quoting |
| `$name` | word at the position of `name` in the `arguments:` frontmatter list |
| `${JCODE_SKILL_DIR}`, `${CLAUDE_SKILL_DIR}` | the skill's directory |
| `${JCODE_PROJECT_DIR}`, `${CLAUDE_PROJECT_DIR}` | the session's project root |

Rules:

- Arguments are inserted literally and never expanded again.
- `\$` produces a literal `$`.
- A missing indexed word (`$3`) keeps its placeholder text. A declared name
  with no matching word becomes empty. Undeclared `$names` stay literal.
- A skill with no placeholder is unchanged. The arguments still arrive as the
  user message, as before.
- A persistent skill is announced when it is activated. Invoking it again with
  arguments that render a different body appends the new rendering. An
  identical rendering is not announced twice in a row.

```markdown
---
name: triage
description: Triage an issue
arguments: [issue, repo]
---
Triage issue $issue in $repo. Notes: $ARGUMENTS
```

`/triage 42 jcode` renders `Triage issue 42 in jcode. Notes: 42 jcode`.
