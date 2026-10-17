# Tasks: Suggested Replies with Default Action Focus & 6-Lens Subagent Auto-Self-Review

## 1. Configuration & Environment Hot-Reload (`jcode-config-types`, `jcode-base`)

- [x] 1.1 Add `SuggestedRepliesConfig` (`enabled`, `use_open_gf`, `max_secondary_suggestions`, `confidence_threshold`, `enable_full_auto_explorer`, `six_lens_auto_review`) with lenient deserializers and `1..=5` / `0.0..=1.0` clamping in `crates/jcode-config-types/src/lib.rs` and `serde_lenient.rs`.
- [x] 1.2 Mount `pub suggested_replies: SuggestedRepliesConfig` on `Config` in `crates/jcode-base/src/config.rs`, wire env overrides in `crates/jcode-base/src/config/env_overrides.rs`, register all new env vars in `CONFIG_ENV_KEYS`, and update `default_file.rs` + `display_summary.rs`.

## 2. OpenJev SystemOne Transport & Two-Stage Cascade (`jcode-base`)

- [x] 2.1 Add `JevProvider::OpenJev` (`https://api.openjev.sh/v1/systemone`, `model: "openjev"`, `OPENJEV_API_KEY` / `openjev.env`), `JevPurpose::Suggestions`, `JevClient::for_suggestions()`, and `choice`/`noul` validation in `crates/jcode-base/src/jev.rs` while preserving strict account isolation on HTTP 402.
- [x] 2.2 Implement `crates/jcode-base/src/suggested_replies.rs` (and export in `crates/jcode-base/src/lib.rs`) with `SuggestedRepliesResponse` (`{ defaultAction, secondarySuggestions, branch }`), `ScoredSuggestion`, Iteration 1 (Jev-native macro-branching vs OpenGF `Sidecar` candidate synthesis), Iteration 2 (OpenJev `noul` logit scoring + deterministic fallback), and `distribute_suggestions`.

## 3. TUI Rendering, Keybindings, Mouse & Full-Auto Explorer (`jcode-tui`)

- [x] 3.1 Add Suggested Replies state, focus tracking, background receiver channel, and 500ms `enable_full_auto_explorer` countdown to `App` and `TuiState` (`crates/jcode-tui/src/tui/app.rs`, `crates/jcode-tui/src/tui/mod.rs`, `crates/jcode-tui/src/tui/app/suggested_replies.rs`).
- [x] 3.2 Render the prominent `Default Action` control and vertical stack of rounded secondary suggestion badges (`1..=5`, one row per item) in `chunks[5]` via `crates/jcode-tui/src/tui/ui_inline.rs` with screen `Rect` hit-target recording.
- [x] 3.3 Wire `[Tab]`, `↑`/`↓` arrows, `1`–`5` (`0`) number shortcuts, `[Enter]`, `[Ctrl+Enter]`, `[Esc]` cancellation, and mouse click hit-testing in `crates/jcode-tui/src/tui/app/input.rs`, `crates/jcode-tui/src/tui/app/remote/key_handling.rs`, `crates/jcode-tui/src/tui/app/navigation.rs`, and `crates/jcode-tui/src/tui/redraw_schedule.rs`.
- [x] 3.4 Trigger non-blocking Suggested Replies cascade at turn end in `crates/jcode-tui/src/tui/app/local.rs` (`finish_turn`) and `crates/jcode-tui/src/tui/app/remote/server_events.rs` (`ServerEvent::Done`).

## 4. 6-Lens Subagent Auto-Self-Review (`jcode-tui`)

- [x] 4.1 Add 6-lens subagent review orchestration (`/review 6`, `/autoreview 6`, and `six_lens_auto_review`) in `crates/jcode-tui/src/tui/app/commands_review.rs` and connect its findings to Suggested Replies `Default Action`.

## 5. Unit & Integration Tests

- [x] 5.1 Add unit tests for `distribute_suggestions` (threshold promotion, `1..=5` clamping, NaN filtering, JSON camelCase schema `{ defaultAction, secondarySuggestions, branch }`), `SuggestedRepliesConfig` lenient parsing + `CONFIG_ENV_KEYS`, `JevProvider::OpenJev` SystemOne requests, and TUI `[Tab]` / `[Ctrl+Enter]` / Explorer timer behavior.
