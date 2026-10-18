## MODIFIED Requirements

### Requirement: Two-Stage Macro/Micro Branching with OpenJev / Laya SystemOne and OpenGF
The system SHALL execute a two-stage cascade at turn completion using a structured `SuggestedRepliesContext` (`last_user_message`, `last_assistant_message`, `assistant_options`, `pending_todos`, `recent_turns`, `language_is_cyrillic`). Iteration 1 determines the macro conversation branch (via SystemOne `choice`/`noul` when `use_open_gf = false`, or via OpenGF `Sidecar` / configured `[providers.*]` / external OAuth fallback routes with `"store": false` when `use_open_gf = true`), and Iteration 2 scores candidate actions against a structured JSON `state` (`{ branch, last_user_message, last_assistant_message, assistant_options, pending_todos, candidates: { cand_0..cand_N } }`) via the configured SystemOne provider (`JevProvider::Laya` over loopback or trusted Tailscale/private LAN HTTP, or `JevProvider::OpenJev` / `TypeSafe` / `OpenRouter` / `Aimlapi` / `Jcode`). When the active SystemOne provider supports `choice` questions, Iteration 2 SHALL submit both a comparative `best_candidate` (`type: "choice"` over `cand_0..cand_N`) and individual `cand_0..cand_N` (`type: "noul"`) questions in a single batch request, combining `noul`, `choice.probabilities`, and rank prior in `calibrate_micro_score`.

#### Scenario: Default Action promotion and secondary suggestions distribution
- **WHEN** candidate actions are scored (via joint `choice + noul` SystemOne evaluation or deterministic local scoring) and sorted by `confidence` descending
- **THEN** if the top candidate has `confidence >= confidence_threshold`, it is assigned to `defaultAction` and up to `max_secondary_suggestions` remaining candidates are placed in `secondarySuggestions`; otherwise `defaultAction` is `None` and up to `max_secondary_suggestions` candidates are placed in `secondarySuggestions`.

#### Scenario: Structured JSON state and joint choice+noul micro-scoring batch
- **WHEN** Iteration 2 evaluates candidate replies on a provider that supports `choice` (`Laya`, `OpenJev`, `TypeSafe`, `OpenRouter`, `Aimlapi`)
- **THEN** the request `state` is a structured JSON object containing `candidates.cand_0..cand_N` (plus conversation and `pending_todos` context), the `questions` map contains `best_candidate` (`type: "choice"`) alongside `cand_0..cand_N` (`type: "noul"`), and `calibrate_micro_score` fuses the `noul` probability, `best_candidate` softmax probability, and rank prior.

## ADDED Requirements

### Requirement: Assistant Question Option Extraction and Language-Aware Dynamic Candidate Pool
The system SHALL inspect `last_assistant_message` for explicit questions or numbered/bulleted options and detect the user's conversation language (Cyrillic/Russian vs English), feeding extracted options and `pending_todos` into both the OpenGF prompt and the deterministic local candidate builder so fallback or `use_open_gf = false` evaluations produce context-relevant replies in the user's language.

#### Scenario: Russian conversation or assistant option list without OpenGF
- **WHEN** `use_open_gf = false` (or OpenGF routes are unreachable) and the recent user turn is in Russian or the assistant message ends with numbered/bulleted options or pending `todo` items
- **THEN** the candidate pool prioritizes direct responses to the assistant's options, the next pending `todo` item, and Russian-localized workflow actions rather than static English-only strings.
