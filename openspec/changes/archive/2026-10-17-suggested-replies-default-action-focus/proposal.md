# Proposal: Suggested Replies with Default Action Focus & 6-Lens Subagent Auto-Self-Review

## Why

After an assistant turn finishes in `jcode`, developers frequently execute predictable next steps (running tests, inspecting diffs, fixing review findings, updating specs, or continuing autonomous exploration). Requiring manual typing for every follow-up slows iteration.

Combining **OpenJev SystemOne** (`https://api.openjev.sh/v1/systemone`, calibrated logit decisions via `choice`/`noul`), **OpenGF** (`Sidecar` candidate synthesis with `"store": false`), and **6-Lens Subagent Auto-Self-Review** lets `jcode`:
1. Classify the macro branch and score next-step candidates asynchronously without blocking the TUI or polluting provider KV caches.
2. Promote the top action (`confidence >= confidence_threshold`) into a **Default Action** button above a vertical stack of `1..=5` secondary capsules, navigable via `[Tab]`, `↑`/`↓`, `1`–`5`, `[Enter]`, and `[Ctrl+Enter]`.
3. Support a cancelable 500ms **Full-Auto Explorer** countdown (`enable_full_auto_explorer`) and **6-Lens Self-Review** (`/review 6`, `/autoreview 6`, `six_lens_auto_review`).

## What Changes

1. **Configuration (`crates/jcode-config-types/src/lib.rs`, `crates/jcode-base/src/config.rs`, `env_overrides.rs`, `default_file.rs`, `display_summary.rs`)**:
   - Add `SuggestedRepliesConfig` (`[suggested_replies]`) with lenient deserializers:
     - `enabled: bool` (default: `true`)
     - `jev_provider: String` (default: `"auto"`, supporting `"auto"`, `"laya"`, `"openjev"`, `"typesafe"`, `"openrouter"`, `"aimlapi"`, `"jcode"`)
     - `use_open_gf: bool` (default: `false`)
     - `max_secondary_suggestions: usize` (default: `5`, clamped `1..=5`)
     - `confidence_threshold: f32` (default: `0.55`, clamped `0.0..=1.0`)
     - `enable_full_auto_explorer: bool` (default: `false`)
     - `six_lens_auto_review: bool` (default: `false`)
   - Register all corresponding `JCODE_*`, `OPENJEV_API_KEY`, and `LAYA_*` environment variables in `CONFIG_ENV_KEYS` so `ConfigCacheFingerprint` hot-reloads cleanly.

2. **OpenJev & Laya SystemOne Transport (`crates/jcode-base/src/jev.rs`)**:
   - Add `JevProvider::OpenJev` (`https://api.openjev.sh/v1/systemone`, model `"openjev"`, `OPENJEV_API_KEY` / `openjev.env`), `JevProvider::Laya` (`http://127.0.0.1:8080/v1/systemone` or `JCODE_LAYA_BASE_URL` / `LAYA_BASE_URL`, model `"laya-latest"`, optional `LAYA_API_KEY` / `laya.env`), and `JevPurpose::Suggestions` (`JevClient::for_suggestions()`).
   - Support and validate `type: "choice"` (with options and probabilities) and `type: "noul"` questions on direct SystemOne routes (`OpenJev`, `TypeSafe`) and batched `noul` questions on all routes.
   - Preserve `jev.rs` account-isolation invariants: when `OPENJEV_API_KEY` is explicitly configured and returns `HTTP 402` (or `401`/`403`), fail fast without spending another paid BYOK account, and let `suggested_replies` degrade gracefully to the deterministic local scorer.

3. **Two-Stage Cascade & Distribution Engine (`crates/jcode-base/src/suggested_replies.rs`)**:
   - Define `SuggestedRepliesResponse` (`#[serde(rename_all = "camelCase")]`: `{ defaultAction: Option<String>, secondarySuggestions: Vec<String>, branch: String }`) and `ScoredSuggestion`.
   - Implement Iteration 1 (Macro-branching via OpenJev `choice` when `use_open_gf == false`, or OpenGF `Sidecar` candidate generation when `use_open_gf == true`) and Iteration 2 (Micro-branching logit scoring via OpenJev `noul` probabilities with deterministic local fallback).
   - Implement `distribute_suggestions` with NaN/Inf filtering, `[0.0, 1.0]` clamping, descending `total_cmp` sorting, `confidence_threshold` promotion to `defaultAction`, and `max_secondary_suggestions` (`1..=5`) capping.

4. **6-Lens Subagent Auto-Self-Review (`crates/jcode-tui/src/tui/app/commands_review.rs`)**:
   - Extend `/review` (`/review 6` / `/review lenses`) and `/autoreview` (`/autoreview 6`) plus `six_lens_auto_review` config to coordinate 6 read-only review lenses (Correctness & Edge Cases, Architecture & Boundaries, Security & Invariants, TUI/UX & Keybindings, Performance/Async & KV-Cache, Tests & Go/No-Go Closure) and synthesize findings into Suggested Replies.

5. **TUI Panel, Vertical Layout, Keybindings, Mouse & Explorer Timer (`crates/jcode-tui/src/tui/{ui_inline.rs, ui.rs, app/suggested_replies.rs, app/input.rs, app/remote/key_handling.rs, app/navigation.rs, redraw_schedule.rs}`)**:
   - Render the Suggested Replies panel inside `chunks[5]` (`inline_ui_height` / `draw_inline_ui`) when idle and input is empty, preserving `chunks[7..=9]` indices, with secondary suggestion capsules (`1..=5`) stacked vertically one per row below `Default Action`.
   - Support `[Tab]` and `↑`/`↓` (cycle focus across `Default Action` and secondary capsules `1..=5` when input is empty), `[Enter]` (submit whichever item is focused), `1`–`5` (and `0` for `Default Action`) for immediate numeric selection and submission, `[Ctrl+Enter]` (immediately submit `Default Action`), and mouse click hit-testing on `Default Action` and secondary capsules `1..=5`.
   - Implement the 500ms cancelable `enable_full_auto_explorer` timer in local and remote tick loops while keeping `redraw_interval` at `REDRAW_IDLE`.

## Capabilities

### New Capabilities
- `suggested-replies-cascade`: Two-stage macro/micro branching and confidence distribution using OpenJev SystemOne (`api.openjev.sh/v1/systemone`) and OpenGF (`Sidecar`).
- `tui-default-action-focus`: TUI rendering in `chunks[5]`, `[Tab]` / `[Ctrl+Enter]` focus and activation, mouse hit-testing for secondary badges, and 500ms Full-Auto Explorer timer.
- `six-lens-auto-review`: Automatic and on-demand 6-lens subagent self-review integrated with turn completion and Suggested Replies.

## Impact

- Crates affected: `jcode-config-types`, `jcode-base`, `jcode-tui`.
- Backward-compatible config and wire formats; zero impact on provider KV-cache prefixes (`Sidecar` runs with `store: false` in background tasks).
