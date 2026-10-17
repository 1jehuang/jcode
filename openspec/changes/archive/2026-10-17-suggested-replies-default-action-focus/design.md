# Design: Suggested Replies with Default Action Focus & 6-Lens Auto-Self-Review

## Context

This design incorporates all findings and conditions from the 6-subagent Go/No-Go architecture review (`panda`, `peacock`, `penguin`, `shark`, `sheep`, `sloth`) against the `jcode` Rust codebase.

## Architecture & Component Design

```
┌────────────────────────────────────────────────────────────────────────────┐
│                   TURN COMPLETION (Local & Remote TUI)                     │
│  `local::finish_turn` (local.rs:624) / `ServerEvent::Done` (server_events) │
└───────────────────┬────────────────────────────────────┬───────────────────┘
                    │                                    │
   (if `six_lens_auto_review`                            │ (async background task)
    and turn modified code/specs)                        ▼
                    ▼                     ┌──────────────────────────────────┐
┌───────────────────────────────────────┐ │ ITERATION 1: Macro-Branching     │
│ 6-LENS SUBAGENT SELF-REVIEW           │ │ • use_open_gf=false:             │
│ 1. Correctness & Edge Cases           │ │   OpenJev `choice` / batched     │
│ 2. Architecture & Module Boundaries   │ │   `noul` over macro branches     │
│ 3. Security & Safety Invariants       │ │ • use_open_gf=true:              │
│ 4. TUI / UX & Keybindings             │ │   OpenGF (`Sidecar`, store=false)│
│ 5. Performance, Async & KV-Cache      │ │   generates branch + 3..6        │
│ 6. Tests & Go/No-Go Closure           │ │   context-specific candidates    │
└───────────────────┬───────────────────┘ └──────────────┬───────────────────┘
                    │ (findings / verdict)               │
                    └────────────────────►┌──────────────▼───────────────────┐
                                          │ ITERATION 2: Micro-Scoring       │
                                          │ POST api.openjev.sh/v1/systemone │
                                          │ `type: "noul"` probabilities     │
                                          │ (+ deterministic fallback on 402)│
                                          └──────────────┬───────────────────┘
                                                         │
                                                         ▼
┌────────────────────────────────────────────────────────────────────────────┐
│              DISTRIBUTION (`distribute_suggestions`)                       │
│ • Filter `!confidence.is_finite()`, clamp `0.0..=1.0`, sort DESC           │
│ • If `top.confidence >= confidence_threshold` (0.55):                      │
│     `defaultAction = Some(top.text)`                                       │
│     `secondarySuggestions = rest.take(max_secondary_suggestions.clamp(1,5))│
│   Else:                                                                    │
│     `defaultAction = None`                                                 │
│     `secondarySuggestions = all.take(max_secondary_suggestions.clamp(1,5))`│
└─────────────────────────────────────┬──────────────────────────────────────┘
                                      │
                                      ▼
┌────────────────────────────────────────────────────────────────────────────┐
│               TUI RENDER & INTERACTION (`chunks[5]` in `ui.rs`)            │
│ • Rendered via `ui_inline.rs` when `inline_ui_state().is_none()`           │
│ • Secondary suggestions `1..=5` stacked vertically (one row per capsule)   │
│ • `[Tab]` & `↑`/`↓` cycle focus across `Default Action` and `1..=5`        │
│ • `[Enter]` submits focused item; `1`–`5` (`0`) & `[Ctrl+Enter]` submit    │
│ • Mouse click on `Default Action` or capsules `1..=5` submits immediately  │
│ • `enable_full_auto_explorer`: 500ms timer (`REDRAW_IDLE`), canceled by Esc│
└────────────────────────────────────────────────────────────────────────────┘
```

## Key Decisions & Review Conditions Enforced

### 1. Configuration & Hot-Reload (`panda` conditions)
- **Dedicated Section**: Define `SuggestedRepliesConfig` in `crates/jcode-config-types/src/lib.rs` and attach `pub suggested_replies: SuggestedRepliesConfig` to `Config` in `crates/jcode-base/src/config.rs`. Also support top-level / `[features]` / `[agents]` aliases where helpful.
- **Lenient Deserialization**: Use custom lenient deserializers in `crates/jcode-config-types/src/serde_lenient.rs` so invalid types or out-of-range numbers in `~/.jcode/config.toml` never break `Config::load_from_file_strict()` during daemon reload handoffs. Clamp `max_secondary_suggestions` to `1..=5` (default `5`) and `confidence_threshold` to `0.0..=1.0` (default `0.55`).
- **Env Fingerprint**: Register `JCODE_SUGGESTED_REPLIES_ENABLED`, `JCODE_USE_OPEN_GF`, `JCODE_MAX_SECONDARY_SUGGESTIONS`, `JCODE_CONFIDENCE_THRESHOLD`, `JCODE_ENABLE_FULL_AUTO_EXPLORER`, `JCODE_SIX_LENS_AUTO_REVIEW`, and `OPENJEV_API_KEY` in `CONFIG_ENV_KEYS` (`crates/jcode-base/src/config.rs:29-210`) so `config_env_fingerprint_tracks_every_apply_env_override_var` passes.

### 2. OpenJev SystemOne Protocol & Account Isolation (`peacock` conditions)
- **Provider Addition**: Add `JevProvider::OpenJev` in `crates/jcode-base/src/jev.rs` with:
  - `name()` -> `"openjev"`
  - `credentials()` -> `("OPENJEV_API_KEY", "openjev.env")`
  - `model()` -> `"openjev"`
  - `endpoint()` -> `"https://api.openjev.sh/v1/systemone"`
- **Question Validation**: Extend `request_body_for` and `validate_answers` in `jev.rs` so `JevPurpose::Suggestions` allows both `choice` and `noul` questions on direct SystemOne endpoints (`OpenJev`, `TypeSafe`), and validates `choice` answers (`answer["choice"]` in criteria keys, finite `confidence` / `probabilities` in `0.0..=1.0`). When routed through `JevProvider::Jcode` (which only accepts `noul` outside Browser), Iteration 1 automatically encodes macro-branch candidates as a single batched `noul` request.
- **Strict Account Isolation**: If `OPENJEV_API_KEY` is present and returns HTTP `402` (or `401`/`403`), `JevClient` fails fast and never falls back to another paid BYOK account (`TYPESAFE_API_KEY` / `OPENROUTER_API_KEY`). The `suggested_replies` engine catches the error and falls back to its built-in deterministic context scorer.

### 3. OpenGF (`Sidecar`) & Distribution Engine (`penguin` conditions)
- **Non-Blocking Execution**: When `use_open_gf == true`, spawn candidate generation on a background tokio task using `Sidecar::new().complete(...)` (`"store": false`), delivering the result to `App` via an `mpsc::unbounded_channel` polled in `handle_tick` (matching `turn_memory.rs`).
- **Deterministic Distribution**: `distribute_suggestions(branch, candidates, confidence_threshold, max_secondary_suggestions)` filters out blank textos and non-finite scores, clamps scores to `0.0..=1.0`, deduplicates case-insensitively keeping highest confidence, and sorts with `b.confidence.total_cmp(&a.confidence)`.

### 4. TUI Layout in `chunks[5]` (`shark` conditions)
- **Zero Chunk-Index Shift**: Do not insert new layout chunks before `chunks[7]` (input), `chunks[8]` (overscroll), or `chunks[9]` (donut). Instead, render the Suggested Replies panel in `chunks[5]` (`inline_ui_height` / `draw_inline_ui` in `crates/jcode-tui/src/tui/ui_inline.rs`) whenever `app.inline_ui_state().is_none()` and `app.suggested_replies_visible()`.
- **Visibility Gate**: `suggested_replies_visible()` is true only when:
  - `!app.is_processing()`
  - `app.input().is_empty()`
  - `!app.onboarding_welcome_active()`
  - `app.inline_ui_state().is_none()`
  - `app.prompt_history_search().is_none()`
  - `app.command_suggestions().is_empty()`
- **Vertical Capsule Stack**: Render the `Default Action` row at the top and stack secondary suggestion capsules (`1..=5`) vertically below it (one capsule per row), using terminal-safe rounded caps (`◖`/`◗` with `▐`/`▌` fallback) matching `ui_onboarding.rs`.

### 5. Keyboard & Mouse Parity (`sheep` conditions)
- **`KeyCode::Tab` & `↑`/`↓` Arrows**: In both `input.rs` (local) and `remote/key_handling.rs` (remote), when `app.suggested_replies_visible()`:
  - `[Tab]` cycles focus forward across `Default Action` -> `1` -> `2` -> ... -> `N` -> `Default Action` (and immediately submits if there is only a single item and it is already focused).
  - `Down` focuses the first item or advances focus to the next suggestion; `Up` moves focus to the previous suggestion across `Default Action` and `1..=5` when suggestion focus is active.
- **`[Enter]`, `[Ctrl+Enter]`, and Number Keys `1`–`5` (`0`)**:
  - When `Default Action` or any secondary suggestion (`1..=5`) is focused and `app.input.is_empty()`, plain `Enter` copies that focused suggestion into `app.input` and submits it through the normal local/remote send pipeline.
  - Pressing `1`–`5` on an empty input immediately focuses and submits the corresponding secondary suggestion (`1..=5`); pressing `0` immediately submits `Default Action` when present.
  - `Ctrl+Enter` (`is_alternate_enter`) when `app.input.is_empty()` immediately activates and submits the `Default Action` even without prior focus.
- **Mouse Click Hit-Testing**: Record screen `Rect`s for the `Default Action` banner and each vertical secondary badge `1..=5` during `draw_inline_ui`. In `navigation.rs` (`handle_mouse_event`), check hit-testing on `MouseEventKind::Down(MouseButton::Left)` before starting chat/input copy selection.

### 6. Full-Auto Explorer & 6-Lens Self-Review Lifecycle (`sloth` conditions)
- **Turn-End Trigger**: In `local::finish_turn` and `remote/server_events.rs` (`ServerEvent::Done`), trigger Suggested Replies generation only when `!followup_scheduled` / `!auto_poked` and `queued_messages.is_empty()`.
- **500ms Explorer Timer**: When `enable_full_auto_explorer == true` and `default_action.is_some()`, arm `explorer_auto_deadline = Some(Instant::now() + Duration::from_millis(500))`. Keep `redraw_schedule.rs` at `REDRAW_IDLE` (`250ms`, bypassing `REDRAW_DEEP_IDLE`) while armed. Any key press (`Esc`, typing, paste, voice) immediately cancels `explorer_auto_deadline`.
- **6-Lens Auto-Self-Review**: Add `build_six_lens_review_startup_message` and `/review 6` / `/autoreview 6` in `commands_review.rs` to orchestrate the 6 review lenses and surface a high-confidence `Default Action` when findings are reported.
