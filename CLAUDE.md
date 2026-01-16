# jcode Development Guidelines

## Workflow

- **Commit as you go** - Make small, focused commits after completing each feature or fix
- **Push when done** - Push all commits to remote when finishing a task or session
- **Test before committing** - Run `cargo test` and `cargo build --release`

## Testing Changes

This repo has self-dev mode. When running `jcode` in this directory:
- It auto-detects the jcode repo and enables self-dev mode
- Builds and tests a canary version before running
- Use `/reload` to hot-reload after making changes

## Commands

```bash
cargo build --release   # Build (auto-updates jcode symlink)
cargo test              # Run all tests
cargo test --test e2e   # Run only e2e tests
```

## Environment Setup

The Claude provider requires the Claude Agent SDK Python bridge:

```bash
export JCODE_CLAUDE_SDK_PYTHON=~/.venv/bin/python3
```

## Key Files

- `src/main.rs` - Entry point, CLI, self-dev mode
- `src/tui/app.rs` - TUI application state and logic
- `src/tui/ui.rs` - UI rendering
- `src/tool/` - Tool implementations
- `src/id.rs` - Session naming and IDs
