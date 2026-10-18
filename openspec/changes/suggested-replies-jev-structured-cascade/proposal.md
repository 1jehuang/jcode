## Why

The current Suggested Replies two-stage cascade sends a flat concatenated transcript string to Jev (`/v1/systemone`) and evaluates each candidate in Iteration 2 via isolated binary `noul` questions, while falling back to static English boilerplate when `use_open_gf = false`. Upgrading the cascade to use a structured JSON `state` (with active todos, last assistant question/options, and candidate dictionary), joint `choice + noul` SystemOne scoring in a single batch, assistant-question option extraction, language-matched dynamic local candidates, user-defined constant candidate questions (`custom_prompts`), per-row model confidence badges (`show_confidence`), a default of 3 suggestions that scales to whatever count is configured in `max_secondary_suggestions`, and session-version caching with a pooled OpenGF HTTP client substantially improves suggestion relevance, ranking discrimination, user customization, and latency.

## What Changes

- **Trajectory 1: Joint `choice + noul` Micro-Scoring in Iteration 2**:
  - Combine a comparative `best_candidate` (`type: "choice"` over `cand_0..cand_N`, yielding softmax `probabilities` and a direct winner) with individual `cand_0..cand_N` (`type: "noul"`) questions in a single `/v1/systemone` batch request when the active Jev provider supports `choice` (`Laya`, `OpenJev`, `TypeSafe`, `OpenRouter`, `Aimlapi`).
  - Fuse `noul` (absolute actionability), `choice.probabilities` (relative head-to-head preference across candidates), and generative rank prior in `calibrate_micro_score_with_choice` so top candidates separate cleanly instead of clustering around `0.58..0.61`.
- **Trajectory 2: Structured JSON `state` for SystemOne & OpenGF (`SuggestedRepliesContext`)**:
  - Replace the flat transcript string with a structured `SuggestedRepliesContext` (`last_user_message`, `last_assistant_message`, `assistant_options`, `pending_todos`, `recent_turns`, and positional `candidates: { cand_0..cand_N }`), matching the proven `memory_jev.rs` and `voice_intent.rs` SystemOne contract where `noul`/`choice` questions reference `state.candidates.cand_i` without duplicating text across question instructions.
- **Trajectory 3: Dynamic Bilingual Candidate Pool, User Constant Questions (`custom_prompts`), and Configurable Suggestion Count (Default 3)**:
  - Change the default `max_secondary_suggestions` (alias `max_suggestions`) from `5` to **`3`**, while removing the hardcoded `5`-item ceiling in `SuggestedRepliesConfig`, `suggested_replies.rs`, and TUI rendering/focus navigation (`ui_inline.rs`, `suggested_replies.rs`) so the system displays as many suggestions as configured (up to the SystemOne batch limit of `20`).
  - Detect the user's conversation language (e.g. Cyrillic/Russian vs English) and synthesize context-aware local candidates from `SuggestedRepliesContext` (extracted assistant options, next pending `todo` item, and localized branch templates).
  - Add `custom_prompts: Vec<String>` (aliases `custom_questions`, `custom_candidates`, `pinned_prompts`, env `JCODE_SUGGESTED_REPLIES_CUSTOM_PROMPTS`) to `[suggested_replies]` so users can configure a constant list of favorite questions/prompts that are merged into the candidate pool and scored by SystemOne (`Laya`/`OpenJev`) on every turn.
- **Trajectory 4: Assistant Question & Option Extraction**:
  - Parse the tail of `last_assistant_message` for explicit questions, numbered/bulleted option lists (`1.`, `2.`, `-`), or slash commands, feeding them into OpenGF as high-priority context and into the deterministic candidate pool as direct answer candidates.
- **Trajectory 5: Per-Row Confidence Display (`show_confidence`), Session-Version Caching, Shared OpenGF Client & Staged Prompt Editing**:
  - Add `show_confidence: bool` (default `true`, env `JCODE_SUGGESTED_REPLIES_SHOW_CONFIDENCE`) to `[suggested_replies]` and display the model's calibrated confidence percentage (`(XX%)`) on **every** suggestion row (`Default Action` and each secondary capsule `1..=N`) when enabled.
  - Reuse a static keep-alive `reqwest::Client` for direct OpenAI-compatible OpenGF profile requests (`complete_via_named_openai_compatible`).
  - Cache computed `SuggestedRepliesResponse` by `(session_id, display_messages_version)` so switching views or re-evaluating an unchanged turn restores suggestions instantaneously.
  - Ensure selecting any suggestion (`Tab`, `↑`/`↓`, `1`–`9`/`0`, or mouse click) stages its full text into `app.input` for user editing while preserving Unicode display-width rendering (`truncate_to_display_width`) and `/suggestions [debug|test]` diagnostics.

## Capabilities

### New Capabilities
<!-- None; this change upgrades existing capabilities -->

### Modified Capabilities
- `suggested-replies-cascade`: Set `max_secondary_suggestions` (alias `max_suggestions`) default to `3` while allowing any configured count (`1..=20`), add `show_confidence` and `custom_prompts` (`custom_questions`) to `SuggestedRepliesConfig`, and upgrade Iteration 1/2 to use structured JSON `state` (`SuggestedRepliesContext`), joint `choice + noul` single-batch micro-scoring on SystemOne (`Laya`/`OpenJev`), assistant question/option extraction, language-aware dynamic fallback candidates, user constant prompt injection, and a shared keep-alive OpenGF HTTP client.
- `tui-default-action-focus`: Render and navigate all configured secondary suggestions (`1..=N`, default 3, instead of hard-capping at 5), display per-row confidence percentages (`(XX%)`) on both `Default Action` and secondary suggestion rows when `show_confidence = true`, stage the selected suggestion into `app.input` for user editing (`Tab`, `↑`/`↓`, `1`–`9`/`0`, mouse click), render pills using Unicode display-column truncation (`truncate_to_display_width`), cache responses by `display_messages_version`, and expose `/suggestions [debug|status|test|refresh]`.

## Impact

- `crates/jcode-config-types/src/lib.rs`, `crates/jcode-base/src/config.rs`, `env_overrides.rs`, `display_summary.rs`: default `max_secondary_suggestions = 3` (clamped `1..=20`, alias `max_suggestions`), `show_confidence`, and `custom_prompts` (`custom_questions`) config fields, lenient deserialization, and `CONFIG_ENV_KEYS` hot-reload tracking.
- `crates/jcode-base/src/suggested_replies.rs`: `SuggestedRepliesContext`, `confidence_for_text`, assistant question/option extractor, localized dynamic candidate builder, `custom_prompts` candidate merging, dynamic candidate count scaling with `max_secondary_suggestions`, structured SystemOne `state`, joint `choice + noul` micro-scoring batch, `calibrate_micro_score_with_choice`, and shared `reqwest::Client`.
- `crates/jcode-tui/src/tui/app/suggested_replies.rs`, `ui_inline.rs`, `input.rs`, `remote/key_handling.rs`, `navigation.rs`: dynamic `1..=N` suggestion rendering and focus cycling (removing `.take(5)`), per-row confidence badge rendering, context extraction (including active `todos`), versioned suggestion cache, staged input editing, Unicode display-width pill rendering, and `/suggestions` diagnostics.
