# Proposal: Suggested Replies with Default Action Focus & 6-Lens Subagent Auto-Self-Review

## Why

After an assistant turn finishes in `jcode`, developers frequently execute predictable next steps: running targeted tests, inspecting diffs, fixing review findings, updating specs, or continuing autonomous repository exploration. Requiring manual typing for every follow-up slows down iteration.

By combining **OpenJev SystemOne** (`https://api.openjev.sh/v1/systemone`, calibrated logit decisions via `choice` and `noul`) with **OpenGF** (generative candidate synthesis via `jcode`'s `Sidecar` with `"store": false`) and a **6-Lens Subagent Auto-Self-Review** pipeline, `jcode` can:
1. Classify the macro conversation branch and score concrete next-step actions at the end of every turn without blocking the TUI or polluting the active provider's KV cache.
2. Promote the highest-confidence action (`confidence >= confidence_threshold`) into a prominent **Default Action** button focused/activated with `[Tab]` or `[Ctrl+Enter]`, while displaying up to `max_secondary_suggestions` (`1..=5`) rounded capsule badges below it.
3. Optionally run in **Full-Auto Explorer** mode (`enable_full_auto_explorer = true`), triggering a 500ms cancelable visual countdown before automatically dispatching the `Default Action` to explore or advance the repository vector based on OpenSpec context.
4. Optionally trigger an automatic **6-Lens Subagent Self-Review** (`six_lens_auto_review = true` or `/review 6` / `/autoreview 6`) across 6 orthogonal read-only lenses (Correctness, Architecture, Security, TUI/UX, Performance/Async, Tests/Go-NoGo) and feed any surfaced findings directly into the Suggested Replies `Default Action`.

## What Changes

1. **Configuration (`crates/jcode-config-types/src/lib.rs`, `crates/jcode-base/src/config.rs`, `env_overrides.rs`, `default_file.rs`, `display_summary.rs`)**:
   - Add `SuggestedRepliesConfig` (`[suggested_replies]`) with lenient deserializers:
     - `enabled: bool` (default: `true`)
     - `use_open_gf: bool` (default: `false`)
     - `max_secondary_suggestions: usize` (default: `5`, clamped `1..=5`)
     - `confidence_threshold: f32` (default: `0.55`, clamped `0.0..=1.0`)
     - `enable_full_auto_explorer: bool` (default: `false`)
     - `six_lens_auto_review: bool` (default: `false`)
   - Register all corresponding `JCODE_*` and `OPENJEV_API_KEY` environment variables in `CONFIG_ENV_KEYS` so `ConfigCacheFingerprint` hot-reloads cleanly.

2. **OpenJev SystemOne Transport (`crates/jcode-base/src/jev.rs`)**:
   - Add `JevProvider::OpenJev` (`https://api.openjev.sh/v1/systemone`, model `"openjev"`, `OPENJEV_API_KEY` / `openjev.env`) and `JevPurpose::Suggestions` (`JevClient::for_suggestions()`).
   - Support and validate `type: "choice"` (with options and probabilities) and `type: "noul"` questions on direct SystemOne routes (`OpenJev`, `TypeSafe`) and batched `noul` questions on all routes.
   - Preserve `jev.rs` account-isolation invariants: when `OPENJEV_API_KEY` is explicitly configured and returns `HTTP 402` (or `401`/`403`), fail fast without spending another paid BYOK account, and let `suggested_replies` degrade gracefully to the deterministic local scorer.

3. **Two-Stage Cascade & Distribution Engine (`crates/jcode-base/src/suggested_replies.rs`)**:
   - Define `SuggestedRepliesResponse` (`#[serde(rename_all = "camelCase")]`: `{ defaultAction: Option<String>, secondarySuggestions: Vec<String>, branch: String }`) and `ScoredSuggestion`.
   - Implement Iteration 1 (Macro-branching via OpenJev `choice` when `use_open_gf == false`, or OpenGF `Sidecar` candidate generation when `use_open_gf == true`) and Iteration 2 (Micro-branching logit scoring via OpenJev `noul` probabilities with deterministic local fallback).
   - Implement `distribute_suggestions` with NaN/Inf filtering, `[0.0, 1.0]` clamping, descending `total_cmp` sorting, `confidence_threshold` promotion to `defaultAction`, and `max_secondary_suggestions` (`1..=5`) capping.

4. **6-Lens Subagent Auto-Self-Review (`crates/jcode-tui/src/tui/app/commands_review.rs`)**:
   - Extend `/review` (`/review 6` / `/review lenses`) and `/autoreview` (`/autoreview 6`) plus `six_lens_auto_review` config to coordinate 6 read-only review lenses (Correctness & Edge Cases, Architecture & Boundaries, Security & Invariants, TUI/UX & Keybindings, Performance/Async & KV-Cache, Tests & Go/No-Go Closure) and synthesize findings into Suggested Replies.

5. **TUI Panel, Keybindings, Mouse & Explorer Timer (`crates/jcode-tui/src/tui/{ui_inline.rs, ui.rs, app/suggested_replies.rs, app/input.rs, app/remote/key_handling.rs, app/navigation.rs, redraw_schedule.rs}`)**:
   - Render the Suggested Replies panel inside `chunks[5]` (`inline_ui_height` / `draw_inline_ui`) when idle and input is empty, preserving `chunks[7..=9]` indices.
   - Support `[Tab]` (focus/activate `Default Action` when input is empty), `[Enter]` (when `Default Action` is focused), `[Ctrl+Enter]` (activate `Default Action` when input is empty), numeric/pill shortcuts, and mouse click hit-testing on the `Default Action` banner and secondary capsule badges `1..=5`.
   - Implement the 500ms cancelable `enable_full_auto_explorer` timer in local and remote tick loops while keeping `redraw_interval` at `REDRAW_IDLE`.

## Capabilities

### New Capabilities
- `suggested-replies-cascade`: Two-stage macro/micro branching and confidence distribution using OpenJev SystemOne (`api.openjev.sh/v1/systemone`) and OpenGF (`Sidecar`).
- `tui-default-action-focus`: TUI rendering in `chunks[5]`, `[Tab]` / `[Ctrl+Enter]` focus and activation, mouse hit-testing for secondary badges, and 500ms Full-Auto Explorer timer.
- `six-lens-auto-review`: Automatic and on-demand 6-lens subagent self-review integrated with turn completion and Suggested Replies.

## Impact

- Crates affected: `jcode-config-types`, `jcode-base`, `jcode-tui`.
- Backward-compatible config and wire formats; zero impact on provider KV-cache prefixes (`Sidecar` runs with `store: false` in background tasks).
