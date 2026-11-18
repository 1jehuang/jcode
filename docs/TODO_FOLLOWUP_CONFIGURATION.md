# Todo follow-up configuration

The aggregate turn-end weak-point reminder is **disabled by default**, including for existing configurations that omit the setting. The display-only **Todo completion** report does not depend on this toggle or on auto-poke.

To opt in, add the following to `~/.jcode/config.toml` (or your `JCODE_HOME/config.toml`):

```toml
[features]
todo_gate_digest = true
```

Set `todo_gate_digest = false` to disable automatic quality reviews again. Auto-poke must also be enabled for a model follow-up to run (`features.auto_poke`, or `/poke on`). `/poke off` does not hide completion reports. Incomplete-work pokes are independent of the quality-review toggle.

## Daemon ownership

For daemon-backed sessions, the daemon owns the follow-up policy for every client, including local TUI, SSH/cloud, mobile and SDK connections. Clients display progress and send `/poke` controls, not their own continuation messages. A stopped model response is an internal turn boundary, not yet a completed harness request. The daemon checks continuation before publishing `Done`, ready status or a finished mobile push. Continuation runs under the same execution lease and terminal identity as the original request.

The host's configuration supplies the initial default. Per-session `/poke on` and `/poke off` controls are persisted, and attaching another client does not override them. `/poke off` can be sent while the agent is busy and stops subsequent continuation, without cancelling the current model response. Explicit cancellation still stops the current request.

Standalone `jcode run` creates an in-process Agent rather than submitting its turns to the daemon. It retains its local continuation loop. A daemon started for its swarm tools does not own that Agent.

With auto-poke enabled, an unchanged incomplete todo list receives at most three automatic continuation attempts. A change to the incomplete list or a new todo cycle resets that budget. The daemon persists its remaining budget across reloads and client disconnects. Explicitly turning auto-poke off still stops these retries.

Three conditions exempt unfinished work from automatic continuation: a todo `waiting_on` background tasks that are still running, a todo marked `requires_user_auth` (**✋ Needs your login**) until the user completes a login or other authentication the agent has no access to, and every todo of a goal whose `cannot_complete` reason is set. None of them marks work completed or spends the retry budget, and independent actionable todos still continue normally. There is no item-level "blocked" state and no timed blocker review. See "Pausing a todo" in `docs/HARNESS.md`.

When enabled, recorded todo gate observations produce the existing aggregate reminder once per completion cycle, after open work finishes and before the completion gates. Repeated observations during that cycle do not trigger another aggregate reminder. New open work starts another cycle. The TUI and headless `jcode run` both respect this setting.

Standalone embeddings that own an Agent without a daemon may use the pure shared policy and should apply their configuration explicitly. Daemon clients must not run a second copy of this loop:

```rust
let mut policy = jcode_sdk::todo::FollowUpPolicy::new(config.features.auto_poke);
policy.set_gate_digest_enabled(config.features.todo_gate_digest);
```

The shared policy also defaults to no digest. `reset_cycle()`, `enable()`, and `disable()` preserve its explicitly configured digest toggle. Apply the returned observation-clearing effects through the client's normal acknowledgment path.
