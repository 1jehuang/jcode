# Skills

jcode loads `SKILL.md` skills from several directories. Each skill is a folder
`<name>/SKILL.md` with YAML frontmatter (`name`, `description`, optional
`allowed-tools`).

## Locations and precedence

Project skills beat global skills. Within one scope, jcode-native directories
beat compatibility directories when two skills share a name:

| Scope | Highest priority first |
|---|---|
| Project | `.jcode/skills`, `.agents/skills`, `.claude/skills` |
| Global | `~/.jcode/skills`, `~/.agents/skills`, Claude plugin store |

When a skill shadows another with the same name, jcode logs one warning naming
both files.
