## Context

`jcode` uses `JevClient` (`crates/jcode-base/src/jev.rs`) across four typed SystemOne decision domains:
1. `Memory` (`memory_jev.rs`): passes structured JSON `state = { query, candidates: { candidate_0..N } }` and evaluates up to 24 `noul` questions per batch.
2. `Browser` (`browser_jev.rs`): passes structured JSON `state` (`page`, `task_context`, `action_history`) and evaluates a single `choice` question over `2..=255` actions.
3. `Voice` (`voice_intent.rs`): passes structured JSON `state = { policy, transcript, candidates }` and evaluates up to 26 `noul` questions in one batch.
4. `Suggestions` (`suggested_replies.rs`): previously passed a flat concatenated string as `state`, repeated candidate strings inside each `noul` question's `instructions`, and evaluated Iteration 2 using only isolated `noul` questions without comparative `choice` probabilities, `todo` awareness, assistant-question option extraction, user-defined constant questions, per-row confidence badges on secondary items, or localized fallback candidates.

## Goals / Non-Goals

**Goals:**
- Upgrade Iteration 2 micro-scoring to combine a comparative `best_candidate` (`type: "choice"`) and individual `cand_0..cand_N` (`type: "noul"`) questions in a single SystemOne request when `client.supports_choice_questions()` is true and `candidates.len() >= 2`.
- Replace the flat transcript string in both Iteration 1 and Iteration 2 with a structured `SuggestedRepliesContext` (`last_user_message`, `last_assistant_message`, `assistant_options`, `pending_todos`, `recent_turns`, `language_is_cyrillic`) and structured SystemOne JSON `state` (`build_structured_systemone_state`).
- Extract explicit questions and numbered/bulleted options from the tail of `last_assistant_message` and feed them into both OpenGF and the local candidate builder.
- Generate language-matched (Russian/Cyrillic vs English) and context-aware (`pending_todos` + assistant options) fallback candidates when `use_open_gf = false` or when OpenGF routes are unavailable.
- Add `custom_prompts: Vec<String>` (aliases `custom_questions`, `custom_candidates`, `pinned_prompts`, env `JCODE_SUGGESTED_REPLIES_CUSTOM_PROMPTS`) to `SuggestedRepliesConfig` so user-configured constant questions are merged into the candidate pool and scored by SystemOne (`Laya`/`OpenJev`) or the local scorer on every turn.
- Add `show_confidence: bool` (default `true`, env `JCODE_SUGGESTED_REPLIES_SHOW_CONFIDENCE`) to `SuggestedRepliesConfig` and render the calibrated model confidence percentage (`(XX%)`) on **every** suggestion row (`Default Action` and each secondary capsule `1..=5`) when enabled.
- Reuse a process-wide keep-alive `reqwest::Client` for OpenGF named-profile HTTP requests and cache `SuggestedRepliesResponse` by `(session_id, display_messages_version)` (with automatic cache bypass/invalidation on 6-lens findings or manual `/suggestions test`).
- Populate `app.input` (`staged_input`) whenever a suggestion is focused/selected (`Tab`, `↑`/`↓`, `1`–`5`/`0`, or mouse click) so the user can edit the prompt before sending, while rendering pills with Unicode display-column truncation (`truncate_to_display_width`).

**Non-Goals:**
- Changing the wire protocol of `/v1/systemone` or modifying `JevPurpose::Memory`, `JevPurpose::Browser`, or `JevPurpose::Voice`.
- Breaking the existing `SuggestedRepliesResponse` JSON contract (`{ defaultAction, secondarySuggestions, branch }`) or removing existing public entry points (`evaluate_cascade`, `score_locally_from_context`, `build_micro_scoring_noul_questions`).

## Decisions

### 1. Joint `choice + noul` Single-Batch Micro-Scoring (Trajectory 1)
- **Decision**: When `client.supports_choice_questions()` is true (`Laya`, `OpenJev`, `TypeSafe`, `OpenRouter`, `Aimlapi`) and `candidates.len() >= 2` (capped to 12 candidates so `1 + 12 = 13 <= MAX_QUESTIONS`), `build_micro_scoring_questions` includes:
  - `"best_candidate"`: `type: "choice"` whose `criteria` map `"cand_0" .. "cand_{N-1}"` to concise descriptions referencing `state.candidates.cand_{i}`.
  - `"cand_0" .. "cand_{N-1}"`: `type: "noul"` evaluating each `state.candidates.cand_{i}` on absolute immediate value.
- **Rationale**: Live verification against `laya-rl-agent` confirmed that referencing `state.candidates.cand_{i}` inside a structured JSON `state` produces calibrated `noul` scores (`0.65..0.84`) alongside softmax `best_candidate.probabilities` in a single request (`~300` input tokens, `0` output tokens). When `candidates.len() == 1` (since `jev.rs` requires `2..=255` criteria for `choice`) or on `JevProvider::Jcode` (`noul`-only), `build_micro_scoring_questions` omits `"best_candidate"` and sends `noul` only.

### 2. Structured SystemOne `state` & `SuggestedRepliesContext` (Trajectory 2)
- **Decision**: Introduce `SuggestedRepliesContext` in `crates/jcode-base/src/suggested_replies.rs` containing:
  - `transcript_tail: String`
  - `last_user_message: Option<String>`
  - `last_assistant_message: Option<String>`
  - `assistant_options: Vec<String>`
  - `pending_todos: Vec<String>`
  - `language_is_cyrillic: bool`
  Provide `SuggestedRepliesContext::from_transcript(transcript_tail)` so legacy callers of `evaluate_cascade(transcript_tail, ...)` and `score_locally_from_context(transcript_tail, ...)` automatically derive `last_user_message`, `last_assistant_message`, `assistant_options`, and `language_is_cyrillic` even when TUI `pending_todos` are not passed explicitly.

### 3. Assistant Question/Option Extraction, Bilingual Dynamic Fallback & User `custom_prompts` (Trajectories 3 & 4)
- **Decision**:
  - Add `extract_assistant_options(last_assistant_message: &str) -> Vec<String>` and `detect_cyrillic_language(text: &str) -> bool`.
  - Add `merge_custom_prompts(base_candidates: Vec<String>, custom_prompts: &[String]) -> Vec<String>` which appends non-empty, case-insensitively deduplicated `custom_prompts` (up to the 12-candidate micro-scoring cap) so user-configured constant questions are always evaluated by SystemOne (`Laya`/`OpenJev`) or the local scorer alongside dynamic candidates.

### 4. Per-Row Confidence Display (`show_confidence`), Pooled OpenGF Client & Session-Version Cache (Trajectory 5)
- **Decision**:
  - Add `SuggestedRepliesResponse::confidence_for_text(&self, text: &str) -> Option<f32>` (looking up `text` in `scored_candidates`). In `draw_suggested_replies_panel` (`ui_inline.rs`), when `show_confidence` is `true`, render `({conf_pct}%)` on both the `Default Action` row and each secondary suggestion pill (`1. {short} ({conf_pct}%)`); when `show_confidence` is `false`, omit the percentage on both.
  - Store two `OnceLock<reqwest::Client>` instances in `suggested_replies.rs` (one with `.no_proxy()` for `http://` local/private endpoints and one standard client for `https://`).
  - Cache `(String, u64, SuggestedRepliesResponse)` (`(session_id, display_messages_version, response)`) in `SuggestedRepliesRuntime`, bypassing/invalidating the cache when `pending_six_lens_findings.is_some()` or when `/suggestions test` (`force_refresh = true`) is invoked.

## Risks / Trade-offs

- **[Risk] `choice` requires at least 2 options in `jev.rs` (`validate_questions`)** → **Mitigation**: Only include `"best_candidate"` (`type: "choice"`) when `candidates.len() >= 2` and `client.supports_choice_questions()` is true.
- **[Risk] User configures many `custom_prompts`** → **Mitigation**: Deduplicate case-insensitively and cap the combined candidate pool to 12 items so `1 + 12 = 13 <= MAX_QUESTIONS (24)`.
