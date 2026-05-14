# Repository Guidelines

## Development Workflow

- **GitHub is the durable source of truth** - For non-trivial repo work, use GitHub Issues, GitHub Projects, and repo docs as the canonical record of scope, acceptance criteria, blockers, decisions, and status. Keep issues/project fields/linked PRs updated religiously; do not rely on ephemeral chat or local notes as the final state.
- **Commit as you go** - Make small, focused commits after completing each feature or fix
- If the git state is not clean, or there are other agents working in the codebase in parallel, do your best to still commit your work. 
- **Push when done** - Push all commits to remote when finishing a task or session
- **Use fast iteration by default** - Prefer `cargo check`, targeted tests, and dev builds while iterating
- **Rebuild when done** - When you are done making changes, build the source.
- **Bump version for releases** - Update version in `Cargo.toml` when making releases. When cutting a new release, look at all the changes that happened since the last release and determine what the version bump should be ie patch or minor, etc. 
- **Remote builds available** - Use `scripts/remote_build.sh` to offload heavy cargo work to another machine. If your build is terminated, likely is because there are not enough resources on this machine to build. use remote build in that case. Try checking the resource avaliablity on the machine before you run a build. 
- **Protect context aggressively** - Every token is gold. Keep main-thread output to checkpoint decisions and concise evidence; route noisy discovery/raw logs to background tasks, subagents, files, side panels, or cached tool outputs.
- **Delegate substantial independent work** - Prefer subagents/child sessions for parallelizable discovery, deep investigation, or long-running validation.
- **Use one-shot mechanical subagents** - For mechanical validation, status, and publishing work, such as long cargo validations, git status/diff summaries, PR/issue creation checks, and final repo audits, use no-context one-shot subagents and return only compact results to the main session.
- **Background means non-UI** - Run background work in non-interactive, non-focus-stealing jobs. Do not spawn headed terminals or steal window focus unless the user explicitly asks.

## Logs
- Logs are written to `~/.jcode/logs/` (daily files like `jcode-YYYY-MM-DD.log`).

## Debug Socket
- Use the debug socket for runtime level debugging

## Install Notes
- `~/.local/bin/jcode` is the launcher symlink used from `PATH`.
- `~/.jcode/builds/current/jcode` is the active local/source-build channel; self-dev builds and `scripts/install_release.sh` point the launcher here.
- `~/.jcode/builds/stable/jcode` is the stable release channel; `scripts/install.sh` installs this and points the launcher here.
- `~/.jcode/builds/versions/<version>/jcode` stores immutable binaries.
- `~/.jcode/builds/canary/jcode` still exists for canary/testing flows, but it is not the primary self-dev install path.
- On Windows, the equivalents are `%LOCALAPPDATA%\\jcode\\bin\\jcode.exe` for the launcher, `%LOCALAPPDATA%\\jcode\\builds\\stable\\jcode.exe` for stable, and `%LOCALAPPDATA%\\jcode\\builds\\versions\\<version>\\jcode.exe` for immutable installs; `scripts/install.ps1` currently installs the stable channel.
- Ensure `~/.local/bin` is **before** `~/.cargo/bin` in `PATH`.
