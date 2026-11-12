# Todo follow-up configuration

The aggregate turn-end weak-point reminder is **disabled by default**, including for existing configurations that omit the setting. The display-only **Todo completion** report does not depend on this toggle or on auto-poke.

To opt in, add the following to `~/.jcode/config.toml` (or your `JCODE_HOME/config.toml`):

```toml
[features]
todo_gate_digest = true
```

Set `todo_gate_digest = false` to disable it again. Auto-poke must also be enabled for a model follow-up to run (`features.auto_poke`, or `/poke on` in the TUI). `/poke off` does not hide completion reports. This setting does not disable the existing incomplete-todo, long-session review, ownership, completion-confidence, confidence-spike, or final-response follow-ups.

With auto-poke enabled, an unchanged incomplete todo list receives at most three automatic continuation attempts. A change to the incomplete list or a new todo cycle resets that budget. The TUI preserves its remaining budget across self-development reloads. Explicitly turning auto-poke off still stops these retries.

Three conditions exempt unfinished work from automatic continuation: a todo `waiting_on` background tasks that are still running, a todo marked `requires_user_auth` (**✋ Needs your authorization**) until the user authorizes it, and every todo of a goal whose `cannot_complete` reason is set. None of them marks work completed or spends the retry budget, and independent actionable todos still continue normally. There is no item-level "blocked" state and no timed blocker review. See "Pausing a todo" in `docs/HARNESS.md`.

When enabled, recorded todo gate observations produce the existing aggregate reminder once per completion cycle, after open work finishes and before the completion gates. Repeated observations during that cycle do not trigger another aggregate reminder. New open work starts another cycle. The TUI and headless `jcode run` both respect this setting.

SDK clients using the pure shared policy should apply their configuration explicitly:

```rust
let mut policy = jcode_sdk::todo::FollowUpPolicy::new(config.features.auto_poke);
policy.set_gate_digest_enabled(config.features.todo_gate_digest);
```

The shared policy also defaults to no digest. `reset_cycle()`, `enable()`, and `disable()` preserve its explicitly configured digest toggle. Apply the returned observation-clearing effects through the client's normal acknowledgment path.
