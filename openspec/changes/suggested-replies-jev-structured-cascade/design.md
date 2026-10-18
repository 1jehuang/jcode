## Context

`jcode` uses `JevClient` (`crates/jcode-base/src/jev.rs`) across four typed SystemOne decision domains:
1. `Memory` (`memory_jev.rs`): passes structured JSON `state = { query, candidates: { candidate_0..N } }` and evaluates up to 24 `noul` questions per batch.
2. `Browser` (`browser_jev.rs`): passes structured JSON `state` (`page`, `task_context`, `action_history`) and evaluates a single `choice` question over `2..=255` actions.
3. `Voice` (`voice_intent.rs`): passes structured JSON `state = { policy, transcript, candidates }` and evaluates up to 26 `noul` questions in one batch.
4. `Suggestions` (`suggested_replies.rs`): previously passed a flat concatenated string as `state`, repeated candidate strings inside each `noul` question's `instructions`, and evaluated Iteration 2 using only isolated `noul` questions without comparative `choice` probabilities, `todo` awareness, assistant-question option extraction, or localized fallback candidates.

## Goals / Non-Goals

**Goals:**
- Upgrade Iteration 2 micro-scoring to combine a comparative `best_candidate` (`type: "choice"`) and individual `cand_0..cand_N` (`type: "noul"`) questions in a single SystemOne request when `client.supports_choice_questions()` is true.
- Replace the flat transcript string with a structured `SuggestedRepliesContext` (`last_user_message`, `last_assistant_message`, `assistant_options`, `pending_todos`, `recent_turns`, `language_is_cyrillic`) and structured SystemOne JSON `state`.
- Extract explicit questions and numbered/bulleted options from the tail of `last_assistant_message` and feed them into both OpenGF and the local candidate builder.
- Generate language-matched (Russian/Cyrillic vs English) and context-aware (`pending_todos` + assistant options) fallback candidates when `use_open_gf = false` or when OpenGF routes are unavailable.
- Reuse a process-wide keep-alive `reqwest::Client` for OpenGF named-profile HTTP requests and cache `SuggestedRepliesResponse` by `(session_id, display_messages_version)`.
- Populate `app.input` (`staged_input`) whenever a suggestion is focused/selected (`Tab`, `↑`/`↓`, `1`–`5`/`0`, or mouse click) so the user can edit the prompt before sending, while rendering pills with Unicode display-column truncation (`truncate_to_display_width`).

**Non-Goals:**
- Changing the wire protocol of `/v1/systemone` or modifying `JevPurpose::Memory`, `JevPurpose::Browser`, or `JevPurpose::Voice`.
- Breaking the existing `SuggestedRepliesResponse` JSON contract (`{ defaultAction, secondarySuggestions, branch }`).

## Decisions

### 1. Joint `choice + noul` Single-Batch Micro-Scoring (Trajectory 1)
- **Decision**: When `client.supports_choice_questions()` is true (`Laya`, `OpenJev`, `TypeSafe`, `OpenRouter`, `Aimlapi`) and there are `>= 2` candidates, `build_micro_scoring_questions` includes:
  - `"best_candidate"`: `type: "choice"` whose `criteria` map `"cand_0" .. "cand_{N-1}"` to concise descriptions referencing `state.candidates.cand_{i}`.
  - `"cand_0" .. "cand_{N-1}"`: `type: "noul"` evaluating each `state.candidates.cand_{i}` on absolute immediate value.
- **Rationale**: SystemOne's `choice` head computes a normalized softmax distribution (`probabilities.cand_i`) across all candidates simultaneously, while `noul` measures whether each candidate is individually worthwhile (`0.0..=1.0`). Fusing `noul`, `choice_prob`, and rank prior in `calibrate_micro_score_with_choice` gives strong separation between #1 and #2 without adding a second HTTP round-trip. When `candidates.len() == 1` or on `JevProvider::Jcode` (which only supports `noul` outside Browser), it gracefully uses `noul`-only.

### 2. Structured SystemOne `state` & `SuggestedRepliesContext` (Trajectory 2)
- **Decision**: Introduce `SuggestedRepliesContext` in `crates/jcode-base/src/suggested_replies.rs` containing:
  - `transcript_tail: String`
  - `last_user_message: Option<String>`
  - `last_assistant_message: Option<String>`
  - `assistant_options: Vec<String>`
  - `pending_todos: Vec<String>`
  - `language_is_cyrillic: bool`
  Serialize a structured JSON `state` for SystemOne containing `branch`, `last_user_message`, `last_assistant_message`, `assistant_options`, `pending_todos`, and `candidates: { cand_0..cand_N }`.
- **Rationale**: Matches `memory_jev.rs` and `voice_intent.rs`, avoids duplicating candidate strings in both `state` and `instructions`, and gives both OpenGF and Laya visibility into pending `todos` and the assistant's closing question.

### 3. Assistant Question/Option Extraction & Bilingual Dynamic Fallback (Trajectories 3 & 4)
- **Decision**: Add `extract_assistant_options(last_assistant_message: &str) -> Vec<String>` and `detect_cyrillic_language(text: &str) -> bool`.
  - `extract_assistant_options` inspects the last ~15 lines of `last_assistant_message` for numbered items (`1. ...`, `1) ...`), bulleted options under a question, or explicit backticked slash commands (`/opsx:apply`, `/review 6`, etc.) and converts them into concise user reply candidates.
  - `contextual_candidates_for_branch(branch: &str, ctx: &SuggestedRepliesContext) -> Vec<String>` prepends extracted assistant options and a prompt for the top pending `todo` item, followed by Russian-localized branch candidates when `ctx.language_is_cyrillic` is true (or English candidates otherwise).
- **Rationale**: Ensures that even with `use_open_gf = false`, suggestions are in the user's language and directly answer the assistant's closing options or advance the next open `todo`.

### 4. Pooled OpenGF Client & Session-Version Cache (Trajectory 5)
- **Decision**:
  - Store two `OnceLock<reqwest::Client>` instances in `suggested_replies.rs` (one with `.no_proxy()` for `http://` local/private endpoints and one standard client for `https://`), avoiding per-turn TLS/connection pool recreation.
  - Cache `(String, u64, SuggestedRepliesResponse)` (`(session_id, display_messages_version, response)`) in `SuggestedRepliesRuntime` so repeated checks on the same `display_messages_version` reuse the cached response without spawning duplicate tasks.

## Risks / Trade-offs

- **[Risk] `choice` requires at least 2 options in `jev.rs` (`validate_questions`)** → **Mitigation**: Only include `"best_candidate"` (`type: "choice"`) when `candidates.len() >= 2` and `client.supports_choice_questions()` is true.
- **[Risk] Extracted assistant markdown bullets could be long** → **Mitigation**: Clean markdown formatting and bound extracted option candidates to concise actionable replies (`<= 110` characters).
