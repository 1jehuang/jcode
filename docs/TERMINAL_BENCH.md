# Terminal-Bench 2.0 with jcode

This document describes the cleanest currently-working path for running jcode on Terminal-Bench 2.0 through Harbor.

## What is in the repo

- `scripts/jcode_harbor_agent.py`
  - Harbor custom agent adapter for jcode
- `scripts/run_terminal_bench_harbor.sh`
  - helper that wires Harbor to the adapter and a Linux-compatible jcode binary
- `scripts/build_linux_compat.sh`
  - builds a Linux jcode artifact against an older glibc baseline for TB-style containers

## Why the compat binary matters

Many Terminal-Bench task containers use an older glibc than a locally-built host binary. The Harbor adapter should use a Linux binary produced by:

```bash
scripts/build_linux_compat.sh /tmp/jcode-compat-dist
```

The helper script will build it for you automatically if it is missing.

## Auth and model assumptions

The current adapter is designed for:

- OpenAI OAuth auth file at `~/.jcode/openai-auth.json`
- `gpt-5.4`
- high reasoning effort
- priority service tier

Those defaults can be overridden with environment variables.

## Quick start

Assuming Terminal-Bench is already available at `/tmp/terminal-bench-2`:

```bash
scripts/run_terminal_bench_harbor.sh \
  --include-task-name regex-log \
  --n-tasks 1 \
  --n-concurrent 1 \
  --jobs-dir /tmp/jcode-tb2 \
  --job-name regex-log-pilot \
  --yes
```

Or point Harbor directly at the remote dataset:

```bash
scripts/run_terminal_bench_harbor.sh \
  --dataset terminal-bench@2.0 \
  --include-task-name regex-log \
  --n-tasks 1 \
  --n-concurrent 1 \
  --jobs-dir /tmp/jcode-tb2 \
  --job-name regex-log-pilot \
  --yes
```

## Useful environment variables

- `JCODE_HARBOR_BINARY`
  - path to the Linux-compatible jcode binary to upload into the task container
- `JCODE_HARBOR_BINARY_DIR`
  - output directory used when auto-building the compat binary
- `JCODE_HARBOR_OPENAI_AUTH`
  - path to the OpenAI OAuth file
- `JCODE_HARBOR_CA_BUNDLE`
  - optional host CA bundle path to upload into the task container
- `JCODE_TB_MODEL`
  - Harbor model string, default `openai/gpt-5.4`
- `JCODE_TB_PATH`
  - default local Terminal-Bench path, default `/tmp/terminal-bench-2`
- `JCODE_OPENAI_REASONING_EFFORT`
  - default `high`
- `JCODE_OPENAI_SERVICE_TIER`
  - default `priority`

## Notes on fairness and state isolation

The adapter gives each trial a fresh in-container jcode home directory under `/tmp/jcode-home`, so memories and auth state are isolated per trial container.

## Current validation status

This path has already been validated with real Harbor task runs using:

- `regex-log`
- `largest-eigenval`
- `cancel-async-tasks`

All three passed in-container with verifier reward `1.0` during the initial pilot.
