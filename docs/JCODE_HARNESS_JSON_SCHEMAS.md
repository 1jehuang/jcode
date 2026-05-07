# jcode-harness JSON Schemas

This document records the stable machine-readable contracts exposed by `jcode-harness`. Fields may be added in future releases, but existing fields should not be removed or renamed without a migration note.

## `init --json`

Command:

```bash
jcode-harness init --cwd /repo --yes --no-memory-wiki --json
```

Shape:

```json
{
  "root": "/repo",
  "files_written": [
    "/repo/AGENTS.md",
    "/repo/.jcode/INIT_REPORT.md",
    "/repo/.jcode/mcp.json"
  ],
  "files_skipped": [],
  "detected_stack": ["Rust"],
  "next_steps": [
    "Review AGENTS.md and .jcode/INIT_QUESTIONS.md",
    "Run `jcode-harness skills doctor`"
  ]
}
```

Guarantees:

- `root` is the resolved project directory used for initialization.
- `files_written` and `files_skipped` contain project scaffold paths touched or preserved by the command.
- `detected_stack` is derived from local repository files such as `Cargo.toml`, `package.json`, `pyproject.toml`, `go.mod`, and Docker files. It is an empty array when no known stack marker exists.
- `next_steps` is an ordered array of operator-facing follow-up strings.
- The command is deterministic for a given project file set, except generated Markdown contents may include timestamps.
- The command does not initialize model providers, MCP servers, browser, Gmail, or network-backed tools.

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

## `session list --json`

Command:

```bash
jcode-harness session list --json
jcode-harness session list --source jcode --include-test --json
```

Shape:

```json
{
  "status": "ok",
  "command": "session list",
  "offline": true,
  "read_only": true,
  "sessions_dir": "/home/user/.jcode/sessions",
  "loaded_count": 2,
  "discovered_count": 1,
  "session_count": 1,
  "hidden_test_count": 0,
  "include_test": false,
  "source": "jcode",
  "limit": null,
  "sessions": [
    {
      "id": "session_visible",
      "parent_id": null,
      "source": "jcode",
      "short_name": "visible",
      "icon": "🌟",
      "title": "Visible local session",
      "message_count": 2,
      "user_message_count": 1,
      "assistant_message_count": 1,
      "created_at": "2026-05-07T20:00:00+00:00",
      "last_message_time": "2026-05-07T20:05:00+00:00",
      "last_active_at": "2026-05-07T20:06:00+00:00",
      "working_dir": "/repo",
      "model": "gpt-test",
      "provider_key": "openai",
      "status": "closed",
      "status_detail": null,
      "needs_catchup": false,
      "estimated_tokens": 0,
      "is_canary": false,
      "is_debug": false,
      "saved": true,
      "save_label": "fixture",
      "server_name": null,
      "server_icon": null,
      "resume_target": { "kind": "jcode_session", "id": "session_visible" },
      "external_path": null
    }
  ]
}
```

Guarantees:

- `offline` and `read_only` are always `true`; the command scans local transcript metadata and does not start the TUI, model providers, servers, or network-backed integrations.
- `loaded_count` is the total number of session entries loaded by the picker backend before `--source`, `--include-test`, and `--limit` filtering.
- `discovered_count` is the count after `--source` filtering and before test/canary hiding and limit truncation.
- `session_count` equals `sessions.length` after all filters are applied.
- `hidden_test_count` is the number of debug/canary sessions hidden by default for the selected source filter. It is `0` when `--include-test` is used.
- `source` is one of `all`, `jcode`, `claude_code`, `codex`, `pi`, or `opencode`.
- Session entries are sorted by most recent message time descending and expose stable metadata needed by future headless orchestration clients.
- `resume_target.kind` is one of `jcode_session`, `claude_code_session`, `codex_session`, `pi_session`, or `opencode_session`.

## `session spawn --dry-run --json`

Command:

```bash
jcode-harness session spawn "draft the release plan" --cwd /repo --provider openai --model gpt-test --dry-run --json
```

Shape:

```json
{
  "status": "ok",
  "command": "session spawn",
  "offline": true,
  "read_only": true,
  "dry_run": true,
  "executed": false,
  "source": "jcode",
  "goal": "draft the release plan",
  "spawn": {
    "supported_by": "jcode-cli-run",
    "execution_supported_by_harness": false,
    "creates_new_session": true,
    "requires_terminal": false,
    "starts_tui": false,
    "starts_provider": "on_execution",
    "program": "jcode",
    "argv": [
      "jcode",
      "-C",
      "/repo",
      "-p",
      "openai",
      "-m",
      "gpt-test",
      "run",
      "--json",
      "draft the release plan"
    ],
    "cwd": "/repo",
    "cwd_source": "argument",
    "output_mode": "json",
    "provider": "openai",
    "provider_profile": null,
    "model": "gpt-test"
  },
  "safety": {
    "executed": false,
    "writes": false,
    "network_required_for_dry_run": false,
    "credentials_required_for_dry_run": false,
    "note": "Use the returned argv/cwd outside dry-run only after choosing an execution surface."
  }
}
```

Guarantees:

- `session spawn` is dry-run only in `jcode-harness`; omitting `--dry-run` fails before a provider, TUI, network, or credential flow is started.
- `offline`, `read_only`, `dry_run`, and `executed` describe the harness command itself: it validates inputs, prints the safe envelope, and does not create a session.
- `spawn.argv` and `spawn.cwd` are execution hints for an operator-selected surface. They are not executed by the harness.
- `spawn.cwd_source` is `argument` when `--cwd` is supplied and `current_dir` otherwise.
- `spawn.provider` is the validated provider argument or `auto`; `spawn.provider_profile` is mutually exclusive with `spawn.provider`.
- `safety.writes`, `network_required_for_dry_run`, and `credentials_required_for_dry_run` are always `false` for the dry-run report.

## `session attach --dry-run --json`

Command:

```bash
jcode-harness session attach session_visible --dry-run --json
```

Shape:

```json
{
  "status": "ok",
  "command": "session attach",
  "offline": true,
  "read_only": true,
  "dry_run": true,
  "executed": false,
  "source": "jcode",
  "id": "session_visible",
  "session_path": "/home/user/.jcode/sessions/session_visible.json",
  "session_exists": true,
  "journal_path": "/home/user/.jcode/sessions/session_visible.journal.jsonl",
  "journal_exists": false,
  "metadata": {
    "id": "session_visible",
    "display_name": "visible",
    "title": "Visible local session",
    "working_dir": "/repo",
    "model": "gpt-test",
    "provider_key": "openai",
    "status": "closed",
    "stored_message_count": 4,
    "user_message_count": 2,
    "assistant_message_count": 1,
    "saved": true
  },
  "attach": {
    "supported_by": "jcode-cli-resume",
    "execution_supported_by_harness": false,
    "attach_mode": "local_session_resume_surface",
    "requires_terminal": true,
    "starts_tui": true,
    "starts_provider": "on_interaction_or_resume_flow",
    "program": "jcode",
    "argv": ["jcode", "--resume", "session_visible"],
    "cwd": "/repo",
    "cwd_source": "session",
    "live_session_detection": "not_attempted_offline_dry_run"
  },
  "safety": {
    "executed": false,
    "writes": false,
    "network_required_for_dry_run": false,
    "credentials_required_for_dry_run": false,
    "note": "Use the returned argv/cwd outside dry-run only after choosing an execution surface."
  }
}
```

Guarantees:

- `session attach` is dry-run only in `jcode-harness`; omitting `--dry-run` fails before any attach/resume flow is started.
- `offline`, `read_only`, `dry_run`, and `executed` describe the harness command itself: it loads local metadata, prints the safe envelope, and does not start the TUI, provider flow, network-backed integrations, or credential prompts.
- `source` is currently `jcode` only. Imported `claude_code`, `codex`, `pi`, and `opencode` attach support is a future compatibility slice.
- `metadata` reuses the same metadata object as `session show --json` and intentionally excludes transcript content.
- `attach.argv` and `attach.cwd` are execution hints for an operator-selected surface. They are not executed by the harness.
- `attach.live_session_detection` is `not_attempted_offline_dry_run`; this slice does not probe running servers or sockets.
- `safety.writes`, `network_required_for_dry_run`, and `credentials_required_for_dry_run` are always `false` for the dry-run report.

## `session show --json`

Command:

```bash
jcode-harness session show session_visible --json
jcode-harness session show session_visible --preview 3 --json
```

Shape:

```json
{
  "status": "ok",
  "command": "session show",
  "offline": true,
  "read_only": true,
  "source": "jcode",
  "id": "session_visible",
  "session_path": "/home/user/.jcode/sessions/session_visible.json",
  "session_exists": true,
  "journal_path": "/home/user/.jcode/sessions/session_visible.journal.jsonl",
  "journal_exists": false,
  "metadata": {
    "id": "session_visible",
    "parent_id": null,
    "short_name": "visible",
    "display_name": "visible",
    "title": "Visible local session",
    "created_at": "2026-05-07T20:00:00+00:00",
    "updated_at": "2026-05-07T20:05:00+00:00",
    "last_active_at": "2026-05-07T20:06:00+00:00",
    "working_dir": "/repo",
    "model": "gpt-test",
    "provider_key": "openai",
    "provider_session_id": "provider-fixture",
    "reasoning_effort": "medium",
    "status": "closed",
    "status_detail": null,
    "stored_message_count": 4,
    "user_message_count": 2,
    "assistant_message_count": 1,
    "env_snapshot_count": 0,
    "memory_injection_count": 0,
    "replay_event_count": 0,
    "estimated_total_tokens": 0,
    "is_canary": false,
    "testing_build": null,
    "is_debug": false,
    "saved": true,
    "save_label": "fixture",
    "last_pid": null,
    "has_compaction": false,
    "compaction": null
  },
  "preview": {
    "requested": 2,
    "returned": 2,
    "content_truncated_to_chars": 4000,
    "messages": [
      {
        "index": 1,
        "role": "assistant",
        "content": "second visible answer",
        "truncated": false,
        "tool_calls": [],
        "tool": null
      }
    ]
  }
}
```

Guarantees:

- `offline` and `read_only` are always `true`; the command loads local session snapshot/journal files and does not start the TUI, model providers, servers, or network-backed integrations.
- `session show` currently supports local jcode session ids only. Imported `claude_code`, `codex`, `pi`, and `opencode` show support is a future compatibility slice.
- Default output uses `--preview 0`, so `preview.messages` is empty and transcript content is not emitted unless explicitly requested.
- `--preview N` returns the last `N` rendered, visible messages. Internal system reminders remain hidden by the renderer.
- Preview `content` is truncated to `content_truncated_to_chars` characters per rendered message, and `truncated` reports whether truncation occurred.
- `metadata.compaction` intentionally reports compaction counters and presence flags only, not summary text or provider-encrypted content.

## `session resume --dry-run --json`

Command:

```bash
jcode-harness session resume session_visible --dry-run --json
```

Shape:

```json
{
  "status": "ok",
  "command": "session resume",
  "offline": true,
  "read_only": true,
  "dry_run": true,
  "executed": false,
  "source": "jcode",
  "id": "session_visible",
  "session_path": "/home/user/.jcode/sessions/session_visible.json",
  "session_exists": true,
  "journal_path": "/home/user/.jcode/sessions/session_visible.journal.jsonl",
  "journal_exists": false,
  "metadata": {
    "id": "session_visible",
    "display_name": "visible",
    "title": "Visible local session",
    "working_dir": "/repo",
    "model": "gpt-test",
    "provider_key": "openai",
    "status": "closed",
    "stored_message_count": 4,
    "user_message_count": 2,
    "assistant_message_count": 1,
    "saved": true
  },
  "resume": {
    "supported_by": "jcode-cli",
    "execution_supported_by_harness": false,
    "requires_terminal": true,
    "starts_tui": true,
    "starts_provider": "on_interaction_or_resume_flow",
    "program": "jcode",
    "argv": ["jcode", "--resume", "session_visible"],
    "cwd": "/repo",
    "cwd_source": "session"
  },
  "safety": {
    "executed": false,
    "writes": false,
    "network_required_for_dry_run": false,
    "credentials_required_for_dry_run": false,
    "note": "Use the returned argv/cwd outside dry-run only after choosing an execution surface."
  }
}
```

Guarantees:

- `session resume` is dry-run only in `jcode-harness`; omitting `--dry-run` fails before any resume flow is started.
- `offline`, `read_only`, `dry_run`, and `executed` describe the harness command itself: it loads local metadata, prints the safe envelope, and does not start the TUI, provider flow, network-backed integrations, or credential prompts.
- `source` is currently `jcode` only. Imported `claude_code`, `codex`, `pi`, and `opencode` resume support is a future compatibility slice.
- `metadata` reuses the same metadata object as `session show --json` and intentionally excludes transcript content.
- `resume.argv` and `resume.cwd` are execution hints for an operator-selected surface. They are not executed by the harness.
- `resume.cwd_source` is `session` when the saved session has a non-empty `working_dir`; otherwise it is `current_dir`.
- `safety.writes`, `network_required_for_dry_run`, and `credentials_required_for_dry_run` are always `false` for the dry-run report.

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

## `skills scope --json`

Commands:

```bash
jcode-harness skills scope init --json
jcode-harness skills scope set optimization --state blocked --reason "benchmark-only" --json
jcode-harness skills scope list --json
```

Shape:

```json
{
  "policy_path": "/repo/.jcode/skills.scope.json",
  "exists": true,
  "created": true,
  "updated": false,
  "policy": {
    "version": 1,
    "default_state": "visible",
    "skills": [
      {
        "name": "optimization",
        "state": "blocked",
        "reason": "benchmark-only"
      }
    ]
  }
}
```

Policy file shape, stored at `.jcode/skills.scope.json`:

```json
{
  "version": 1,
  "default_state": "visible",
  "skills": [
    { "name": "llmwiki-memory", "state": "discoverable", "reason": "manual provenance only" }
  ]
}
```

Guarantees:

- States are `visible`, `discoverable`, or `blocked`.
- `visible` skills can be selected automatically or explicitly.
- `discoverable` skills are skipped during automatic routing, but allowed when passed with `--skill`.
- `blocked` skills are removed from both automatic and explicit selection.
- `skills match --json` and `jcode-harness run --dry-run` honor this policy before injecting skill prompts.
- Invalid skill names containing path separators, `..`, or unsupported characters are rejected.

## `skills import --json`

Command:

```bash
jcode-harness skills import --json
jcode-harness skills import --from .claude/skills --apply --json
```

Shape:

```json
{
  "status": "ok",
  "offline": true,
  "dry_run": true,
  "root": "/repo",
  "target": {
    "scope": "project",
    "path": "/repo/.jcode/skills"
  },
  "force": false,
  "planned": 1,
  "copied": 0,
  "skipped": 0,
  "errors": 0,
  "warnings": 0,
  "sources": [
    {
      "origin": "agents",
      "path": "/repo/.agents/skills",
      "exists": true,
      "checked": 1
    }
  ],
  "findings": [],
  "actions": [
    {
      "name": "repo-reviewer",
      "source_origin": "agents",
      "source_path": "/repo/.agents/skills/repo-reviewer",
      "target_path": "/repo/.jcode/skills/repo-reviewer",
      "action": "copy",
      "applied": false,
      "findings": []
    }
  ]
}
```

Guarantees:

- `offline` is always `true`; import planning never invokes providers, MCP servers, browser, or Gmail integrations.
- The command is dry-run by default. Files are copied only when `--apply` is passed.
- Default source discovery checks `./.agents/skills`, `./.claude/skills`, `./.codex/skills`, and `./.jcode/skills`; repeated `--from <dir>` values replace the default source set.
- Relative `--from` values are resolved against `--cwd` or the current directory.
- Default target is project scope at `./.jcode/skills`; `--scope global` targets `$JCODE_HOME/skills`.
- Existing target skills are reported as `skip-existing` unless `--force` is passed with `--apply`.
- Import refuses to copy symlinks during apply and reports copy failures as `errors`, causing a non-zero exit after printing JSON.
- Action values currently include `copy`, `overwrite`, `skip-existing`, `skip-invalid`, `skip-same-target`, and `error`.

## `skills validate --json`

Command:

```bash
jcode-harness skills validate --cwd . --json
```

Shape:

```json
{
  "status": "ok",
  "offline": true,
  "root": "/repo",
  "checked": 5,
  "valid": 5,
  "invalid": 0,
  "errors": 0,
  "warnings": 0,
  "origins": [
    {
      "origin": "project-local",
      "path": "/repo/.jcode/skills",
      "exists": true,
      "checked": 1
    }
  ],
  "findings": [
    {
      "severity": "warning",
      "code": "prompt-injection-phrase",
      "origin": "project-local",
      "path": "/repo/.jcode/skills/example/SKILL.md",
      "message": "Skill contains a common prompt-injection phrase; review before enabling it"
    }
  ],
  "skills": [
    {
      "name": "repo-reviewer",
      "description": "Project review rules",
      "origin": "project-local",
      "path": "/repo/.jcode/skills/repo-reviewer/SKILL.md",
      "valid": true,
      "effective": true,
      "precedence_rank": 30,
      "allowed_tools": ["read", "grep"],
      "issues": []
    }
  ]
}
```

Guarantees:

- `offline` is always `true`; validation never invokes providers, MCP servers, browser, or Gmail integrations.
- Standard origins are checked in runtime precedence order: built-in, `./.claude/skills`, `$JCODE_HOME/skills`, then `./.jcode/skills`.
- `status` is `error` when invalid skill files are found, `warn` when only warnings exist, and `ok` when there are no warning/error findings.
- The command exits non-zero when `errors > 0`, while still printing the JSON report to stdout for CI parsers.
- `findings` includes normalized `severity`, stable `code`, `origin`, `path`, and human-readable `message` fields.
- `skills[].effective` marks the final highest-precedence valid definition when the winner is deterministic.
- Current validation errors include missing/invalid frontmatter and unsupported `allowed-tools` shapes; comma-separated strings and YAML lists are accepted. Warnings include empty bodies, directory/name mismatches, same-precedence duplicates, common prompt-injection phrases, suspicious inline secrets, and risky shell snippets.

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
  ],
  "policy": {
    "policy_path": "/repo/.jcode/skills.scope.json",
    "policy_exists": true,
    "selected": [
      { "name": "repo-reviewer", "state": "visible", "explicit": true, "selected": true }
    ],
    "skipped": [
      {
        "name": "optimization",
        "state": "blocked",
        "explicit": false,
        "selected": false,
        "reason": "state is blocked by project skill scope policy",
        "policy_reason": "benchmark-only"
      }
    ]
  }
}
```

Guarantees:

- `selected` preserves router order: explicit `--skill` values first, followed by automatic matches.
- Resolved entries use the shared skill entry shape after source precedence resolution.
- Missing explicit skills are reported as `{ "name": "...", "missing": true }` instead of failing, so automation can decide whether to block.
- `--cwd` changes repo-local skill resolution without requiring a provider call.
- `policy` records the repo-local scope-policy decision for every raw selected skill.

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
      "name": "wiki_search",
      "purpose": "Find literal text across wiki pages and optionally raw session transcripts.",
      "when_to_use": "When exact wording, issue numbers, or command output matters.",
      "mcp_tool": "mcp__llmwiki__wiki_search",
      "example": { "term": "llmwiki-memory", "include_raw": false }
    },
    {
      "name": "wiki_read_page",
      "purpose": "Read one known wiki or raw page by path for provenance.",
      "when_to_use": "After query/search returns a source path that needs verification.",
      "mcp_tool": "mcp__llmwiki__wiki_read_page",
      "example": { "path": "wiki/index.md" }
    },
    {
      "name": "wiki_sync",
      "purpose": "Import new local agent session transcripts into raw/sessions for future wiki use.",
      "when_to_use": "At explicit memory-capture checkpoints after reviewing local write/secret boundaries.",
      "mcp_tool": "mcp__llmwiki__wiki_sync",
      "example": { "dry_run": true },
      "write_risk": "local-files"
    },
    {
      "name": "wiki_export",
      "purpose": "Export a machine-readable wiki index or flattened dump for handoff/context packaging.",
      "when_to_use": "When producing durable handoff context or release evidence.",
      "mcp_tool": "mcp__llmwiki__wiki_export",
      "example": { "format": "llms-txt" }
    },
    {
      "name": "wiki_lint",
      "purpose": "Check wiki graph health, broken wikilinks, stale summaries, and contradictions.",
      "when_to_use": "Before trusting wiki context in a release or long-running agent loop.",
      "mcp_tool": "mcp__llmwiki__wiki_lint",
      "example": {}
    }
  ],
  "recommended_flow": [
    "Run wiki_query with the task question.",
    "Use wiki_search for exact issue numbers or command names.",
    "Read cited pages with wiki_read_page before treating them as evidence.",
    "Use wiki_sync --dry-run first when capturing new local transcripts.",
    "Run wiki_lint before release or handoff if wiki-derived context is relied on."
  ]
}
```

Guarantees:

- This command is a deterministic offline preview and never invokes MCP tools itself.
- `offline` is always `true` and `network_required` is always `false` for the preview command.
- Every command entry includes `name`, `purpose`, `when_to_use`, `mcp_tool`, and `example`.
- Commands that can write local files when invoked externally include an explicit `write_risk` field.
- `permission_boundary` records the read/write/secret constraints that automation should surface before using the concrete MCP tools.

## `demo --json`

Command:

```bash
jcode-harness demo --cwd /repo --json
```

Shape:

```json
{
  "status": "ok",
  "offline": true,
  "network_required": false,
  "credentials_required": false,
  "root": "/repo",
  "demos": [
    {
      "id": "mock-provider-run-json",
      "surface": "mock-provider",
      "title": "Exercise the real Agent runtime with a deterministic provider response.",
      "claim": "The run JSON contract can be parsed without network, model credentials, or quota.",
      "command": "jcode-harness run 'review this diff' --json --mock-response 'mocked harness response'",
      "argv": ["jcode-harness", "run", "review this diff", "--json", "--mock-response", "mocked harness response"],
      "offline": true,
      "network_required": false,
      "credentials_required": false,
      "project_writes": false,
      "expected_evidence": ["provider is harness-mock"],
      "notes": "May write normal session metadata under JCODE_HOME, but does not write project files."
    }
  ],
  "recommended_flow": [
    "Run safe-eval first in unfamiliar repositories."
  ]
}
```

Guarantees:

- The manifest is deterministic for a given `--cwd` and does not execute the listed demos.
- Top-level `offline`, `network_required`, and `credentials_required` describe the manifest command itself and are always `true`, `false`, and `false` respectively.
- Every `demos[]` entry includes stable `id`, `surface`, `title`, `claim`, copy-paste `command`, structured `argv`, booleans for `offline`, `network_required`, `credentials_required`, and `project_writes`, plus non-empty `expected_evidence`.
- Current surfaces include `safe-eval`, `mock-provider`, `memory`, `plan`, `swarm`, `browser`, `skills`, and `release-gates`.
- Entries with `project_writes: true` are intended for temporary or safe-eval workspaces; the manifest still does not write those files by itself.

## `demo run <id|all> --json`

Command:

```bash
jcode-harness demo run mock-provider-run-json --cwd /repo --json
jcode-harness demo run all --cwd /repo --json
jcode-harness demo run all --cwd /repo --sandbox --json
```

Shape:

```json
{
  "status": "ok",
  "offline": true,
  "network_required": false,
  "credentials_required": false,
  "root": "/repo",
  "execution_root": "/repo",
  "sandbox": {
    "enabled": false,
    "path": null,
    "retained": false,
    "cleanup": "none"
  },
  "requested": "mock-provider-run-json",
  "allow_writes": false,
  "results": [
    {
      "id": "mock-provider-run-json",
      "surface": "mock-provider",
      "status": "pass",
      "exit_code": 0,
      "executed_root": "/repo",
      "project_writes": false,
      "command": "jcode-harness run 'review this diff' --json --mock-response 'mocked harness response'",
      "json_parseable": true,
      "stdout": "{ ... }",
      "stderr": ""
    }
  ]
}
```

Guarantees:

- The runner only executes commands from the local deterministic `demo --json` manifest.
- `project_writes: true` demos are blocked by default and reported as `status: "blocked"`; pass `--allow-writes` only in a disposable or safe-eval workspace.
- `--sandbox` creates a temporary execution root, rewrites demo `--cwd` values to that root, allows `project_writes: true` demos there, and removes the sandbox by default after rendering the JSON report.
- `--keep-sandbox` requires `--sandbox` and leaves the sandbox directory available for manual inspection.
- `demo run all --json` executes non-writing demos and reports writing demos as blocked, returning `status: "warn"` when blocks are the only non-pass results.
- A single blocked demo exits non-zero after printing the JSON report so CI can fail safely.
- `json_parseable` is true only when the child command was expected to emit JSON and stdout parsed successfully.

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

## `clean-code check --json`

Command:

```bash
jcode-harness clean-code check --json --fail-on warning src
```

Shape:

```json
{
  "root": "/repo",
  "files_scanned": 1,
  "findings": [
    {
      "rule_id": "no-silent-error-swallowing",
      "severity": "error",
      "path": "src/main.rs",
      "line": 42,
      "message": "Result from fallible operation is ignored; handle or propagate the error",
      "snippet": "let _ = std::fs::read_to_string(path);"
    }
  ],
  "rules_loaded": 5
}
```

Guarantees:

- The command is an offline deterministic quality gate and does not call model providers or network-backed tools.
- `root` is the cwd or explicit `--cwd` used to resolve relative scan paths.
- `files_scanned` counts supported source files actually read. Unsupported files and skipped directories are omitted.
- `findings` is an array sorted by scan order and includes stable `rule_id`, normalized lowercase `severity`, repo-relative `path` when possible, 1-based `line`, human-readable `message`, and source `snippet`.
- `severity` is one of `info`, `warning`, or `error`.
- `rules_loaded` is the number of built-in Clean Code Guardian rules loaded for the scan.
- Exit status is non-zero when any finding severity is at or above `--fail-on`, while the JSON report is still printed to stdout for CI parsers.
- `clean-code rules` intentionally emits YAML, not JSON. Parseability is covered separately by the rule-pack tests.

## Compatibility policy

- Additive fields are allowed.
- Removing or renaming fields requires a migration note and release-gate update.
- Consumers should tolerate unknown fields.
- Tests in `tests/e2e/harness_cli.rs` cover parseability and required fields for the current schemas.
