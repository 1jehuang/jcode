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

## Discover providers and current selection

List provider IDs you can pass to `-p/--provider`:

```bash
jcode --quiet provider list
jcode --quiet provider list --json
```

Inspect the currently requested and resolved provider/model selection:

```bash
jcode --quiet provider current
jcode --quiet --provider openai --model gpt-5.4 provider current --json
```

Verbose human summary:

```bash
jcode --quiet model list --verbose
```

## Run one prompt and return JSON

```bash
jcode --quiet run --json "Reply with exactly OK"
```

## Stream one prompt as NDJSON

```bash
jcode --quiet run --ndjson "Reply with exactly OK"
```

Typical event types:

- `start`
- `connection_phase`
- `connection_type`
- `text_delta`
- `text_replace`
- `tool_start`
- `tool_input`
- `tool_exec`
- `tool_done`
- `tokens`
- `service_tier`
- `done`
- `error`

The final `done` event includes the assembled text and usage summary.

### Service-tier evidence

When the provider reports a tier, `service_tier` events include `requested`,
`tier` (the raw provider report), and `report_status`. The final `done` event
also includes `requested_service_tier`, `reported_service_tier`, and
`service_tier_report_status`. Status values compare metadata only:

- `not_reported`: no tier report was received.
- `unresolved`: the provider reported `auto`, not a resolved tier.
- `reported_match`: the requested and reported tier agree, including known aliases.
- `reported_mismatch`: the requested and reported tier differ.

`served_service_tier` remains a compatibility alias for `reported_service_tier`.
Despite its older name, it is **not independent verification** of backend
routing or billing. Subscription-backed Codex responses can report `default`
even when controlled measurements show different speed behavior for requested
tiers. Do not infer a confirmed downgrade, an applied Ultrafast tier, or a billing
rate from throughput or that field alone. Preserve the raw report and request
separately. Provider support can correlate requests using the allowlisted
`request_id` / `oai_request_id` fields in Jcode's stream lifecycle logs.

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

## Inspect build/version details

```bash
jcode --quiet version
jcode --quiet version --json
```

JSON output includes:

- `version`
- `git_hash`
- `git_tag`
- `build_time`
- `git_date`
- `release_build`

## Notes

- JSON commands are designed so the intended machine-readable result is printed to `stdout`
- With `--quiet`, wrapper-oriented commands should keep `stderr` empty unless there is a real warning/error
- `jcode model list` and `jcode run --json` do not require the TUI
- `jcode model list` does not require an already-running shared server
