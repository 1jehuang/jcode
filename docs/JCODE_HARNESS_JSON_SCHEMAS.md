# jcode-harness JSON Schemas

This document records the stable machine-readable contracts exposed by `jcode-harness`. Fields may be added in future releases, but existing fields should not be removed or renamed without a migration note.

## `safe-eval --json`

Command:

```bash
jcode-harness safe-eval --json
```

Shape:

```json
{
  "profile": "safe-eval",
  "root": "/repo",
  "jcode_home": "/repo/.jcode/safe-eval/home",
  "runtime_dir": "/repo/.jcode/safe-eval/home/runtime",
  "env_file": "/repo/.jcode/safe-eval/safe-eval.env",
  "powershell_env_file": "/repo/.jcode/safe-eval/safe-eval.ps1",
  "guide_file": "/repo/.jcode/safe-eval/README.md",
  "source_command": "source '/repo/.jcode/safe-eval/safe-eval.env'",
  "powershell_command": ". '/repo/.jcode/safe-eval/safe-eval.ps1'",
  "env": [
    { "name": "JCODE_HOME", "value": "/repo/.jcode/safe-eval/home" },
    { "name": "JCODE_SAFE_EVAL", "value": "1" },
    { "name": "JCODE_NO_TELEMETRY", "value": "1" }
  ],
  "disabled_surfaces": ["telemetry", "ambient autonomous cycles"],
  "files_written": ["/repo/.jcode/safe-eval/safe-eval.env"],
  "files_skipped": []
}
```

Guarantees:

- `profile` is always `safe-eval`.
- `env` lists the environment variables written to both activation files.
- `source_command` is a POSIX shell activation hint; `powershell_command` is a PowerShell activation hint.
- `files_written` and `files_skipped` are absolute or cwd-relative paths matching the operator-provided `--cwd`/`--home` values.
- The command is deterministic and does not contact model providers or start MCP/browser/Gmail integrations.

## `doctor --json`

Command:

```bash
jcode-harness doctor --json
```

Shape:

```json
{
  "status": "warn",
  "offline": true,
  "root": "/repo",
  "platform": { "os": "linux", "arch": "x86_64" },
  "jcode_home": {
    "path": "/repo/.jcode/safe-eval/home",
    "source": "env",
    "exists": true
  },
  "safe_eval": {
    "active": true,
    "active_marker": true,
    "active_home_matches_profile": true,
    "profile_dir": "/repo/.jcode/safe-eval",
    "expected_home": "/repo/.jcode/safe-eval/home",
    "files": [
      { "name": "posix_env", "path": "/repo/.jcode/safe-eval/safe-eval.env", "exists": true }
    ]
  },
  "privacy": {
    "jcode_no_telemetry": true,
    "do_not_track": true,
    "telemetry_opted_out": true
  },
  "features": {
    "ambient_enabled_env": "false",
    "swarm_enabled_env": "false",
    "memory_backend_env": "off"
  },
  "skills": { "status": "ok", "builtins": 4, "loaded": 4 },
  "mcp": {
    "configs": [
      {
        "scope": "project-jcode",
        "path": "/repo/.jcode/mcp.json",
        "exists": false,
        "requires_review": false
      }
    ]
  },
  "recommendations": []
}
```

Guarantees:

- `offline` is always `true`; the command does not initialize providers or start MCP/browser/Gmail integrations.
- `status` is `ok` when there are no recommendations, otherwise `warn`.
- `safe_eval` reports both the explicit `JCODE_SAFE_EVAL=1` marker and whether active `JCODE_HOME` matches the generated profile home.
- `mcp.configs` reports candidate config paths and marks project-local configs as `requires_review` only when they exist.
- `recommendations` is an array of operator-facing strings suitable for onboarding checklists.

## Shared skill entry

Used by `skills list --json`, `skills show <name> --json`, `skills doctor --json`, and resolved entries in `skills match <goal> --json`.

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

## `skills match <goal> --json`

Command:

```bash
jcode-harness skills match "fix this Rust bug" --skill repo-reviewer --json
```

Shape:

```json
{
  "goal": "fix this Rust bug",
  "mode": "auto",
  "selected": [
    {
      "name": "repo-reviewer",
      "description": "Repo review policy",
      "origin": "project-local",
      "path": "/repo/.jcode/skills/repo-reviewer/SKILL.md",
      "allowed_tools": null
    },
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

- `selected` preserves router order: explicit `--skill` values first, followed by automatic matches.
- Resolved entries use the shared skill entry shape after source precedence resolution.
- Missing explicit skills are reported as `{ "name": "...", "missing": true }` instead of failing, so automation can decide whether to block.
- `--cwd` changes repo-local skill resolution without requiring a provider call.

## `skills llmwiki-bridge --json`

Command:

```bash
jcode-harness skills llmwiki-bridge --json
```

Shape:

```json
{
  "skill": "llmwiki-memory",
  "kind": "local-mcp-bridge-preview",
  "offline": true,
  "network_required": false,
  "permission_boundary": {
    "default": "read-only preview; this command never invokes MCP tools",
    "writes": "wiki_sync may write local raw/session pages only when the operator explicitly invokes it outside this preview",
    "secrets": "do not record credentials, tokens, private keys, or unredacted personal data in wiki pages"
  },
  "commands": [
    {
      "name": "wiki_query",
      "purpose": "Retrieve synthesized project memory, decisions, and prior context by question.",
      "when_to_use": "Before planning or coding when prior decisions may exist.",
      "mcp_tool": "mcp__llmwiki__wiki_query",
      "example": { "question": "what did we decide about embedded skills?", "max_pages": 5 }
    },
    {
      "name": "wiki_sync",
      "purpose": "Import new local agent session transcripts into raw/sessions for future wiki use.",
      "when_to_use": "At explicit memory-capture checkpoints after reviewing local write/secret boundaries.",
      "mcp_tool": "mcp__llmwiki__wiki_sync",
      "example": { "dry_run": true },
      "write_risk": "local-files"
    }
  ],
  "recommended_flow": [
    "Run wiki_query with the task question.",
    "Use wiki_search for exact issue numbers or command names."
  ]
}
```

Guarantees:

- This command is a deterministic offline preview and never invokes MCP tools itself.
- `offline` is always `true` and `network_required` is always `false` for the preview command.
- Every command entry includes `name`, `purpose`, `when_to_use`, `mcp_tool`, and `example`.
- Commands that can write local files when invoked externally include an explicit `write_risk` field.
- `permission_boundary` records the read/write/secret constraints that automation should surface before using the concrete MCP tools.

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
