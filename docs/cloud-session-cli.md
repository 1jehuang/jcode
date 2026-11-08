# Cloud session CLI

These commands operate on sessions migrated with `jcode cloud move` or `/cloud`.
They use the session's recorded cloud host rather than silently starting a new
local copy. Sign in to the same Jcode account on the CLI and mobile app.

```bash
# Keep the original conversation local and migrate a separate child.
jcode cloud fork --session <local-session> --prompt "Continue this task"

# Start a detached turn on a session that already lives on cloud.
jcode cloud send --session <cloud-session> "Run the tests and fix failures"

# Inspect progress or wait for the detached turn to finish.
jcode cloud status --session <cloud-session> --json
jcode cloud wait --session <cloud-session> --timeout 300 --json
jcode cloud logs --session <cloud-session> --lines 100

# Diagnose the cloud path and review recent handoff failures.
jcode cloud doctor --json
jcode cloud problems --limit 5 --json
```

`wait` exits with code 2 when its timeout expires. A timeout does not cancel the
remote turn. Transport and inspection errors are failures, not a successful idle
result. A failed remote turn also makes `wait` exit unsuccessfully. Detached
turns require an inactive session: an already-active daemon agent is rejected
rather than allowing two processes to write the same conversation. The active
owner must release the session before a detached turn can start. Detached
execution requires a Unix cloud host. `doctor` is read-only: it must not
provision or wake a sleeping managed
machine merely to inspect its health. Skipped checks are reported explicitly.

In the TUI, `/mobile` migrates the session and its swarm to the managed Jcode
Cloud host, then requests a push notification targeting that session. If the
session is already on cloud, it requests only the notification. Tap the push in
the mobile app to open the conversation. A push failure does not undo a completed
migration, and `/mobile cancel` only cancels a queued handoff.

The hidden `inspect` and `run-detached` subcommands are the remote protocol used
by the public lifecycle commands. They are not a bypass for migration ownership
or session concurrency checks.
