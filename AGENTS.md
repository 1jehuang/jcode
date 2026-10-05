# Repository Guidelines

## Repository Scope

- Jcode Desktop is in a separate repository.

## Development Workflow

- **Use the user's Git identity** - Create commits with the configured
  `user.name` and `user.email`. Do not override them with `Jcode`, `Jcode agent`,
  or a fabricated agent email. Preserve existing contributor attribution when
  integrating work. If no identity is configured, ask rather than inventing one.
- **Welcome pull requests from everyone** - Review contributions on their merits,
  regardless of whether the author is a maintainer, an existing contributor, a
  first-time contributor, or an agent. Good PRs can be merged directly after review
  and validation. Do not require a maintainer-authored rewrite merely because of
  who submitted the change. See `CONTRIBUTING.md` for the contribution policy.
- **Keep work scoped** - Work on your own branch and preserve unrelated work. When
  the user asks you to review or integrate a PR or branch, you may inspect, test,
  and integrate that contribution regardless of author status. Do not pull in
  unrelated branches or merge a PR without user authorization.

## Project Isolation Invariants

One `jcode` daemon serves sessions for **many projects at once**. A project is a session's
working directory. The daemon is long-lived, so anything resolved from process state is
resolved from whichever single project happened to start it. A violation here is a bug
even when it "works" in single-project testing. The tracked backlog is
[`docs/plans/PROJECT_ISOLATION_HARDENING_PLAN.md`](docs/plans/PROJECT_ISOLATION_HARDENING_PLAN.md).

### 1. `working_dir: None` never means the daemon's cwd

The daemon's cwd is one arbitrary project. Never resolve a project-relative resource from
it in a session-scoped path.

- No `working_dir.unwrap_or(Path::new("."))` and no
  `unwrap_or_else(|| std::env::current_dir())` in session-scoped code. With no working dir,
  return an actionable error or use global-only data.
- Keep the process-cwd fallback only in the CLI's own load paths, and name those functions
  so the difference is visible at the call site.
- Decide and test the `None` behavior for every project-scoped resource: skills
 (`crates/jcode-base/src/skill.rs`), `AGENTS.md` and system prompt
 (`crates/jcode-base/src/prompt.rs`), project MCP config (`crates/jcode-app-core/src/tool/mod.rs`),
 memory (`crates/jcode-base/src/memory.rs`), goals (`crates/jcode-base/src/goal.rs`).

### 2. No unscoped global state

Before adding a `static`, `OnceCell`, or `LazyLock`, or writing a new file under `~/.jcode/`,
answer whether it is shared by every project, and whether that is intended.

- Shared-by-default state must be keyed by project, or its cross-project scope documented
 at the definition (ambient and `~/.jcode/config.toml` are documented cases).
- Key pooled and cached resources by project identity, not by name alone. A cache keyed by
 a bare string lets two projects collide silently.
- Derive project keys with one shared helper that canonicalizes the path and uses a stable
 hash. `DefaultHasher` over a raw `PathBuf` is not stable across Rust releases and does not
 normalize case or symlinks, which fragments per-project data on Windows.

### 3. Client-supplied ids are untrusted

The socket is local and unauthenticated, and the session picker intentionally lists sessions
across projects. Cross-project access is a product decision, not an accident.

- A request that names a session must verify the connection owns it. Never dispatch a
 wire-supplied `session_id` straight into a handler; compare it against the connection's own
 session id, or rebase onto that.
- Tools acting on existing objects (background tasks, schedules, applets, sessions) must
 check ownership in the default path. Opt-outs must be explicit and named.
- When a client reports a working directory that differs from an existing session's, pick
 a winner deliberately and apply that one answer everywhere (agent state, swarm identity,
 project-local MCP). Two subsystems choosing different answers is the bug.

### 4. Cross-project data is opt-in, and opt-in means tested

- Default to project scope whenever a filter exists, even if agents rarely need the wider
 view.
- Guard sloppy-filter fallbacks (prefix matching that degrades to substring matching, for
 example) so they cannot quietly widen the result set.
- Anything deliberately global (ambient mode, global memory and goal scope, telemetry
 consent) is fine. Record that it is deliberate in the code so a later reader does not
 re-open it as a suspected bug.

### 5. Comment intent must match the code

Several isolation bugs sat directly under comments claiming the opposite behavior, such as a
"constructed per session" note above a process-wide construction. Update the comment in the
same change that moves scoping. If a comment asserts an isolation guarantee, there should be
 a test that fails when the guarantee breaks.

## Install Notes
- `~/.local/bin/jcode` is the launcher symlink used from `PATH`.
- `~/.jcode/builds/current/jcode` is the active local/source-build channel; self-dev builds and `scripts/install_release.sh` point the launcher here.
- `~/.jcode/builds/stable/jcode` is the stable release channel; `scripts/install.sh` installs this and points the launcher here.
- `~/.jcode/builds/versions/<version>/jcode` stores immutable binaries.
- `~/.jcode/builds/canary/jcode` still exists for canary/testing flows, but it is not the primary self-dev install path.
- On Windows, the equivalents are `%LOCALAPPDATA%\\jcode\\bin\\jcode.exe` for the launcher, `%LOCALAPPDATA%\\jcode\\builds\\stable\\jcode.exe` for stable, and `%LOCALAPPDATA%\\jcode\\builds\\versions\\<version>\\jcode.exe` for immutable installs; `scripts/install.ps1` currently installs the stable channel.
- Ensure `~/.local/bin` is **before** `~/.cargo/bin` in `PATH`.

## Verifying a change at runtime

`cargo build` alone proves nothing about behavior. `jcode run` and interactive
sessions are served by the long-lived daemon at
`~/.jcode/builds/shared-server/jcode`, which is a symlink into
`~/.jcode/builds/versions/<version>/`. Until that symlink is repointed and the
daemon restarted (`jcode self-dev --build`), a freshly built binary is inert and
every runtime check silently measures the old code.

To test a change without disturbing the shared daemon or the caller's session,
run your build against its own socket:

```bash
cargo build --profile selfdev
./target/selfdev/jcode run --no-update --socket /run/user/1000/jcode-mytest.sock '<prompt>'
```

Two things that waste time otherwise:

- `crate::logging::info` writes to a log file, not stderr, so instrumenting a
  code path with it produces no visible output under `--trace`. Use `eprintln!`
  for throwaway diagnostics and delete it before committing.
- Confirm which binary you are actually inspecting. `strings` on
  `builds/shared-server/jcode` reads a 70-byte symlink, not a program; resolve it
  with `readlink -f` first.
