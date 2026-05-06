# jcode-harness JSON Schemas

This document records the stable machine-readable contracts exposed by `jcode-harness`. Fields may be added in future releases, but existing fields should not be removed or renamed without a migration note.

## Shared skill entry

Used by `skills list --json`, `skills show <name> --json`, and `skills doctor --json`.

```json
{
  "name": "karpathy-guidelines",
  "description": "Skill description",
  "origin": "built-in",
  "path": "<builtin>/.jcode/skills/karpathy-guidelines/SKILL.md",
  "allowed_tools": null
}
```

Field meanings:

- `name`: stable skill identifier from frontmatter.
- `description`: frontmatter description.
- `origin`: one of `built-in`, `claude-compat`, `global`, `project-local`, or `unknown`.
- `path`: effective source path. Built-ins use virtual `<builtin>/...` paths.
- `allowed_tools`: `null` or an array of tool names parsed from `allowed-tools` frontmatter.

## `skills list --json`

Command:

```bash
jcode-harness skills list --json
```

Shape:

```json
{
  "skills": [
    {
      "name": "karpathy-guidelines",
      "description": "Skill description",
      "origin": "built-in",
      "path": "<builtin>/.jcode/skills/karpathy-guidelines/SKILL.md",
      "allowed_tools": null
    }
  ]
}
```

Guarantees:

- `skills` is an array sorted by skill name.
- Entries are final effective skills after precedence resolution.

## `skills show <name> --json`

Command:

```bash
jcode-harness skills show karpathy-guidelines --json
```

Shape:

```json
{
  "name": "karpathy-guidelines",
  "description": "Skill description",
  "origin": "built-in",
  "path": "<builtin>/.jcode/skills/karpathy-guidelines/SKILL.md",
  "allowed_tools": null,
  "content": "Markdown body without YAML frontmatter"
}
```

Guarantees:

- Returns the final effective skill for `name`.
- `content` is the markdown body after frontmatter parsing.
- Missing skills exit non-zero with a human-readable error.

## `skills doctor --json`

Command:

```bash
jcode-harness skills doctor --json
```

Shape:

```json
{
  "skills_loaded": 3,
  "builtins": [
    {
      "name": "karpathy-guidelines",
      "status": "ok",
      "path": "<builtin>/.jcode/skills/karpathy-guidelines/SKILL.md"
    }
  ],
  "duplicates": [
    {
      "name": "example-skill",
      "entries": [
        {
          "name": "example-skill",
          "origin": "global",
          "path": "/home/user/.jcode/skills/example-skill/SKILL.md"
        }
      ]
    }
  ],
  "skills": []
}
```

Guarantees:

- `skills_loaded` equals the length of final effective `skills`.
- `builtins` reports required embedded skill availability with `status` `ok` or `missing`.
- `duplicates` reports discovered duplicate names across standard origins before precedence resolution.
- `skills` is the same effective-entry shape as `skills list --json`.

## `run --json`

Command:

```bash
jcode-harness run "review this diff" --json --mock-response "ok"
```

Shape:

```json
{
  "session_id": "session_...",
  "provider": "harness-mock",
  "model": "harness-mock-model",
  "text": "ok",
  "usage": {
    "input_tokens": 1,
    "output_tokens": 1,
    "cache_read_input_tokens": null,
    "cache_creation_input_tokens": null
  }
}
```

Guarantees:

- `text` is the captured assistant response from the Agent runtime.
- `usage` is the last token usage snapshot known to the Agent.
- `--mock-response` is offline and deterministic. Real providers may report different token counts or cache fields.

## `run --ndjson`

Command:

```bash
jcode-harness run "review this diff" --ndjson --mock-response "ok"
```

Shape, one JSON object per line:

```jsonl
{"type":"start","session_id":"session_...","provider":"harness-mock","model":"harness-mock-model"}
{"type":"done","session_id":"session_...","text":"ok","usage":{"input_tokens":1,"output_tokens":1,"cache_read_input_tokens":null,"cache_creation_input_tokens":null}}
```

Guarantees:

- The first event is `type: "start"`.
- The terminal success event is `type: "done"`.
- Future event types may be added between `start` and `done`; consumers should ignore unknown event types unless they opt into them.

## Compatibility policy

- Additive fields are allowed.
- Removing or renaming fields requires a migration note and release-gate update.
- Consumers should tolerate unknown fields.
- Tests in `tests/e2e/harness_cli.rs` cover parseability and required fields for the current schemas.
