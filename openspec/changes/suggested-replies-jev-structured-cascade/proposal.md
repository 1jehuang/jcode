## Why

The current Suggested Replies two-stage cascade sends a flat concatenated transcript string to Jev (`/v1/systemone`) and evaluates each candidate in Iteration 2 via isolated binary `noul` questions, while falling back to static English boilerplate when `use_open_gf = false`. Upgrading the cascade to use a structured JSON `state` (with active todos, last assistant question/options, and candidate dictionary), joint `choice + noul` SystemOne scoring in a single batch, assistant-question option extraction, language-matched dynamic local candidates, and session-version caching with a pooled OpenGF HTTP client substantially improves suggestion relevance, ranking discrimination, and latency.

## What Changes

- **Trajectory 1: Joint `choice + noul` Micro-Scoring in Iteration 2**:
  - Combine a comparative `best_candidate` (`type: "choice"` over `cand_0..cand_N`, yielding softmax `probabilities` and a direct winner) with individual `cand_0..cand_N` (`type: "noul"`) questions in a single `/v1/systemone` batch request when the active Jev provider supports `choice` (`Laya`, `OpenJev`, `TypeSafe`, `OpenRouter`, `Aimlapi`).
  - Fuse `noul` (absolute actionability), `choice.probabilities` (relative head-to-head preference across candidates), and generative rank prior in `calibrate_micro_score` so top candidates separate cleanly instead of clustering around `0.58..0.61`.
- **Trajectory 2: Structured JSON `state` for SystemOne & OpenGF (`SuggestedRepliesContext`)**:
  - Replace the flat transcript string with a structured `SuggestedRepliesContext` (`last_user_message`, `last_assistant_message`, `assistant_questions_or_options`, `pending_todos`, `recent_turns`, and positional `candidates: { cand_0..cand_N }`), matching the proven `memory_jev.rs` and `voice_intent.rs` SystemOne contract where `noul`/`choice` questions reference `state.candidates.cand_i` without duplicating text across question instructions.
- **Trajectory 3: Dynamic Bilingual & Context-Aware Candidate Pool (`use_open_gf = false` / Fallback)**:
  - Detect the user's conversation language (e.g. Cyrillic/Russian vs English) and synthesize context-aware local candidates from `SuggestedRepliesContext` (extracted assistant options, next pending `todo` item, and localized branch templates) so local/fallback mode never emits canned English boilerplate into a Russian conversation.
- **Trajectory 4: Assistant Question & Option Extraction**:
  - Parse the tail of `last_assistant_message` for explicit questions, numbered/bulleted option lists (`1.`, `2.`, `-`), or slash commands, feeding them into OpenGF as high-priority context and into the deterministic candidate pool as direct answer candidates.
- **Trajectory 5: Session-Version Response Caching, Shared OpenGF HTTP Client & Staged Prompt Editing**:
  - Reuse a static keep-alive `reqwest::Client` for direct OpenAI-compatible OpenGF profile requests (`complete_via_named_openai_compatible`).
  - Cache computed `SuggestedRepliesResponse` by `(session_id, display_messages_version)` so switching views or re-evaluating an unchanged turn restores suggestions instantaneously.
  - Ensure selecting any suggestion (`Tab`, `↑`/`↓`, `1`–`5`/`0`, or mouse click) stages its full text into `app.input` for user editing while preserving Unicode display-width rendering (`truncate_to_display_width`) and `/suggestions [debug|test]` diagnostics.

## Capabilities

### New Capabilities
<!-- None; this change upgrades existing capabilities -->

### Modified Capabilities
- `suggested-replies-cascade`: Upgrade Iteration 1/2 to use structured JSON `state` (`SuggestedRepliesContext`), joint `choice + noul` single-batch micro-scoring on SystemOne (`Laya`/`OpenJev`), assistant question/option extraction, language-aware dynamic fallback candidates, and a shared keep-alive OpenGF HTTP client.
- `tui-default-action-focus`: Update TUI selection behavior (`Tab`, `↑`/`↓`, `1`–`5`/`0`, mouse click) to stage the selected suggestion into `app.input` for user editing, render pills using Unicode display-column truncation (`truncate_to_display_width`), cache responses by `display_messages_version`, and expose `/suggestions [debug|status|test|refresh]`.

## Impact

- `crates/jcode-base/src/suggested_replies.rs`: `SuggestedRepliesContext`, assistant question/option extractor, localized dynamic candidate builder, structured SystemOne `state`, joint `choice + noul` micro-scoring batch, `calibrate_micro_score`, and shared `reqwest::Client`.
- `crates/jcode-tui/src/tui/app/suggested_replies.rs`, `ui_inline.rs`, `input.rs`, `remote/key_handling.rs`, `navigation.rs`: context extraction (including active `todos`), versioned suggestion cache, staged input editing, Unicode display-width pill rendering, and `/suggestions` diagnostics.
