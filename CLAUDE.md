# J-Code Project Instructions

## After Making Code Changes

Always rebuild the release binary after making changes so the user has the updated version. Run cargo build as a background task to avoid blocking:

```fish
cargo build --release  # Run this in background
```

The `jcode` command is symlinked to `target/release/jcode`, so rebuilding automatically updates it.

## Running Tests

```fish
cargo test              # Run all tests (unit + e2e)
cargo test --test e2e   # Run only e2e tests
```

## Environment Setup

The Claude provider requires the Claude Agent SDK Python bridge:

```fish
set -x JCODE_CLAUDE_SDK_PYTHON ~/.venv/bin/python3
```
