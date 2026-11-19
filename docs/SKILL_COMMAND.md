# Skill `command:` frontmatter

A `SKILL.md` may declare an optional `command:` key. When a user types
`/skill-name args` in the TUI and the skill has a `command`, jcode runs
`command` followed by the shell-quoted `args` exactly like a `!` input line:
locally in the session working directory, with the output shown in the
transcript. No model turn starts and no tokens are spent.

```markdown
---
name: fleet
description: Manage remote workers with the fleet CLI
command: fleet
---

# fleet

Use `fleet status`, `fleet dispatch <worker> <task>` ...
```

`/fleet dispatch box-1 'fix the build'` runs
`fleet dispatch box-1 'fix the build'`.

Arguments are split with POSIX shell rules and each one is re-quoted, so the
user's quoting is honoured but arguments cannot inject extra shell syntax
(`;`, `$(...)` and so on stay literal). The `command` string itself is trusted
shell, the same as anything else in a skill you installed.

Without `command:` the skill behaves as before (activation plus an optional
trailing prompt). The model-facing `skill_manage` tool still loads the skill
body, so the model can read the documentation and invoke the CLI through
`bash` itself. In a remote/attached client the line runs on the server's
input shell, the same as `!` lines.
