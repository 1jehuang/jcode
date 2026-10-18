## 1. Structured Context, Assistant Option Extraction & Bilingual Candidate Pool (`jcode-base`)

- [ ] 1.1 Define `SuggestedRepliesContext` (`transcript_tail`, `last_user_message`, `last_assistant_message`, `assistant_options`, `pending_todos`, `language_is_cyrillic`) and `detect_cyrillic_language` in `crates/jcode-base/src/suggested_replies.rs`.
- [ ] 1.2 Implement `extract_assistant_options(last_assistant_message: &str) -> Vec<String>` to extract numbered/bulleted options, explicit questions, and slash commands from the closing lines of the assistant turn.
- [ ] 1.3 Implement `contextual_candidates_for_branch(branch: &str, ctx: &SuggestedRepliesContext) -> Vec<String>` combining extracted assistant options, the top pending `todo`, and Russian/English localized branch candidates.

## 2. Structured SystemOne `state`, Joint `choice + noul` Micro-Scoring & Pooled OpenGF Client (`jcode-base`)

- [ ] 2.1 Implement `build_structured_systemone_state(branch: Option<&str>, ctx: &SuggestedRepliesContext, candidates: &[String]) -> Value` so SystemOne receives structured JSON with `candidates.cand_0..cand_N`.
- [ ] 2.2 Implement `build_micro_scoring_questions(branch: &str, candidates: &[String], include_choice: bool) -> Result<Map<String, Value>>` combining `"best_candidate"` (`type: "choice"`) and `"cand_0..cand_N"` (`type: "noul"`) in a single batch when `include_choice && candidates.len() >= 2`.
- [ ] 2.3 Update `calibrate_micro_score_with_choice(raw_noul: f32, choice_prob: Option<f32>, is_choice_winner: bool, idx: usize, from_open_gf: bool) -> f32` and wire it into `evaluate_cascade_with_context`.
- [ ] 2.4 Replace per-call `reqwest::Client::builder()` in `complete_via_named_openai_compatible` with shared `OnceLock<reqwest::Client>` instances (`no_proxy` for HTTP and default for HTTPS) and feed `assistant_options` + `pending_todos` into the OpenGF synthesis prompt.

## 3. TUI Context Builder, Session-Version Cache & Verification (`jcode-tui`)

- [ ] 3.1 Build `SuggestedRepliesContext` in `crates/jcode-tui/src/tui/app/suggested_replies.rs` from `display_messages` and active session `todos` (`pending` / `in_progress`).
- [ ] 3.2 Add `(session_id, display_messages_version)` response caching in `SuggestedRepliesRuntime` and expose structured context details in `/suggestions debug`.
- [ ] 3.3 Add unit tests in `jcode-base` and `jcode-tui` covering assistant option extraction, Russian/Cyrillic dynamic candidates, joint `choice + noul` scoring, and session-version caching, then run `selfdev test` and `selfdev build-reload`.
