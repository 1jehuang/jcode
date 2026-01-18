# jcode Development Guidelines

## Workflow

- **Commit as you go** - Make small, focused commits after completing each feature or fix
- **Push when done** - Push all commits to remote when finishing a task or session
- **No AI co-author** - Never include `Co-Authored-By` lines in commits
- **Rebuild and install when done** - Run `cargo build --release && cp target/release/jcode ~/.local/bin/`
- **Test before committing** - Run `cargo test` to verify changes
- **Bump version for releases** - Update version in `Cargo.toml` when making releases

## Versioning

jcode uses semantic versioning (`vX.Y.Z`). The version is defined in `Cargo.toml`.

- **Patch** (0.1.0 → 0.1.1): Bug fixes
- **Minor** (0.1.0 → 0.2.0): New features, backward compatible
- **Major** (0.1.0 → 1.0.0): Breaking changes

When to bump:
- Bump **patch** for bug fixes and small improvements
- Bump **minor** for new features or significant enhancements
- Bump **major** for breaking changes to CLI, config, or APIs

The build includes git hash and `-dev` suffix for uncommitted changes (e.g., `v0.1.0-dev (abc1234)`).

## Testing Changes

This repo has self-dev mode. When running `jcode` in this directory:
- It auto-detects the jcode repo and enables self-dev mode
- Builds and tests a canary version before running
- Use `/reload` to hot-reload after making changes

**Manual testing** - After making changes, manually test the feature in a real terminal to verify it works. Use kitty to launch test instances:
```bash
sock=$(ls /tmp/kitty.sock* | head -1)
kitten @ --to unix:$sock launch --type=os-window ./target/release/jcode --standalone
```

## Commands

```bash
cargo build --release && cp target/release/jcode ~/.local/bin/  # Build and install
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
