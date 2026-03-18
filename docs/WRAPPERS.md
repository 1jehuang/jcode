# jcode wrapper / scripting guide

This document describes the non-interactive CLI surface intended for wrappers, scripts, and other tools that invoke `jcode`.

## Recommended flags

Use these flags by default in wrappers:

```bash
jcode --quiet --no-update --no-selfdev ...
```

- `--quiet` suppresses non-error CLI/status chatter
- `--no-update` avoids update-check noise/work
- `--no-selfdev` avoids repository auto-detection changing runtime behavior

## Discover available models

List model names that can be passed to `-m/--model`:

```bash
jcode --quiet model list
jcode --quiet model list --json
jcode --quiet --provider openai model list --json
```

Verbose human summary:

```bash
jcode --quiet model list --verbose
```

## Run one prompt and return JSON

```bash
jcode --quiet run --json "Reply with exactly OK"
```

Example shape:

```json
{
  "session_id": "session_...",
  "provider": "OpenAI",
  "model": "gpt-5.4",
  "text": "OK",
  "usage": {
    "input_tokens": 123,
    "output_tokens": 7,
    "cache_read_input_tokens": 0,
    "cache_creation_input_tokens": null
  }
}
```

## Inspect authentication state

```bash
jcode --quiet auth status
jcode --quiet auth status --json
```

JSON output includes:

- `any_available`
- `providers[]`
  - `id`
  - `display_name`
  - `status`
  - `method`
  - `auth_kind`
  - `recommended`

## Notes

- JSON commands are designed so the intended machine-readable result is printed to `stdout`
- With `--quiet`, wrapper-oriented commands should keep `stderr` empty unless there is a real warning/error
- `jcode model list` and `jcode run --json` do not require the TUI
- `jcode model list` does not require an already-running shared server
