# TuiState Trait Decomposition Plan

Status: Analysis + proposed plan

This document audits the `TuiState` trait (`crates/jcode-tui/src/tui/mod.rs`) and
proposes a safe, incremental decomposition. It is the Phase 1.5 follow-on to the
`App` god-object decomposition (see `CLIENT_CORE_PRESENTATION_SPLIT_PLAN.md`).

## Current state

- `pub trait TuiState` exposes **114 methods**.
- Implementors: 2 (`App` in `tui/app/tui_state.rs`, and `TestState` in
  `tui/ui_tests/mod.rs`).
- Consumers: ~95 usages across 29 files, almost all as `&dyn TuiState` (50
  render-function signatures take `app: &dyn TuiState`).

It is the presentation-layer counterpart to the `App` god-object: a single wide
interface that couples every render module to the entire client surface.

## Why a naive sub-trait split has limited value

Two structural facts constrain the refactor:

1. **`App` implements the whole surface regardless.** Splitting `TuiState` into
   `TuiTranscriptState + TuiInputState + ...` does not reduce what `App` must
   implement, and (because the trait is presentation-only data access) it does
   not change crate-level compile coupling. The win is intent/navigability, not
   decoupling of `App`.

2. **`&dyn TuiState` does not compose.** Render functions take trait objects.
   Rust has no stable `&dyn (A + B)`, so any consumer that needs methods from
   more than one domain must take a supertrait that re-aggregates them. The two
   central renderers (`ui.rs`, `ui_viewport.rs`) use methods from nearly every
   domain, so they would keep the full supertrait bound.

Measured: of the ~28 `&dyn TuiState` render modules, only **2** are
multi-category (`ui.rs`, `ui_viewport.rs`); the other ~26 each use a single
domain. So a sub-trait split *does* narrow the declared surface for the majority
of render modules, but the headline god-interface (driven by the 2 central
renderers) stays wide via the supertrait.

Conclusion: the split is worthwhile for readability and for narrowing leaf
render-module bounds, but it is **not** a compile-coupling win and should be done
incrementally to avoid a high-conflict big-bang across 29 files.

## Proposed target shape

```
trait TuiState:
    TuiTranscriptState + TuiInputState + TuiScrollState + TuiStreamStatusState
    + TuiProviderState + TuiSessionServerState + TuiWorkspaceState
    + TuiDiagramPaneState + TuiDiffPaneState + TuiSidePanelState
    + TuiInlineState + TuiOverlayState + TuiCopySelectionState
    + TuiOnboardingState + TuiMiscState
{}
```

`App` and `TestState` keep a single `impl` per sub-trait (mechanical move). The 2
central renderers take `&dyn TuiState` (the supertrait). Each leaf render module
narrows to the one sub-trait it needs.

## Method categorization (all 114)

### TuiTranscriptState
display_messages, display_user_message_count, compacted_hidden_user_prompts,
has_display_edit_tool_messages, side_pane_images, display_messages_version,
render_streaming_markdown

### TuiInputState
input, cursor_pos, queued_messages, interleave_message,
pending_soft_interrupts, has_stashed_input, command_suggestions,
command_suggestion_selected, suggestion_prompts, queue_mode,
next_prompt_new_session_armed, dictation_key_label

### TuiScrollState
scroll_offset, auto_scroll_paused, chat_overscroll_active,
copy_selection_edge_autoscroll_active, chat_native_scrollbar,
has_pending_mouse_scroll_animation

### TuiStreamStatusState
streaming_text, is_processing, streaming_tokens, streaming_cache_tokens,
output_tps, streaming_tool_calls, elapsed, status, active_skill,
subagent_status, batch_progress, time_since_activity, stream_message_ended,
status_notice, status_detail, rate_limit_remaining, animation_elapsed

### TuiProviderState
provider_name, provider_model, upstream_provider, connection_type,
mcp_servers, available_skills, auth_status, update_cost,
total_session_tokens, session_compaction_count, context_info,
context_snapshot, context_limit, cache_ttl_status

### TuiSessionServerState
is_remote_mode, is_canary, is_replay, current_session_id,
session_display_name, server_display_name, server_display_icon,
server_sessions, connected_clients, remote_startup_phase_active,
client_update_available, server_update_available, info_widget_data,
active_experimental_feature_notice

### TuiWorkspaceState
workspace_mode_enabled, workspace_map_rows, workspace_animation_tick

### TuiDiagramPaneState
diagram_mode, diagram_focus, diagram_index, diagram_scroll,
diagram_pane_ratio, diagram_pane_ratio_user_adjusted, diagram_pane_animating,
diagram_pane_enabled, diagram_pane_position, diagram_zoom

### TuiDiffPaneState
diff_mode, diff_pane_scroll, diff_pane_scroll_x, diff_pane_focus,
diff_line_wrap

### TuiSidePanelState
side_panel, side_panel_image_zoom_percent, side_panel_native_scrollbar,
pin_images, pinned_images_auto_hide_remaining_secs

### TuiInlineState
inline_interactive_state, inline_view_state, inline_ui_state

### TuiOverlayState
changelog_scroll, help_scroll, model_status_overlay, session_picker_overlay,
login_picker_overlay, account_picker_overlay, usage_overlay

### TuiCopySelectionState
copy_badge_ui, copy_selection_mode, copy_selection_range,
copy_selection_status

### TuiOnboardingState
onboarding_preview_mode, onboarding_welcome_active, onboarding_welcome_kind

### TuiMiscState
working_dir, now_millis, has_notification, centered_mode

## Incremental, low-conflict migration

Do **not** split all 15 sub-traits at once across 29 files. Recommended order:

1. Land the documented section headers in the trait definition (done; pure
   comments, single file). Gives the categorization a canonical home.
2. Extract one leaf sub-trait with a single-file consumer as a proof of pattern
   (e.g. `TuiCopySelectionState` or `TuiDiagramPaneState`). Verify with
   `cargo check -p jcode-tui`.
3. Extract remaining leaf sub-traits one per commit, narrowing the corresponding
   leaf render module's bound in the same commit.
4. Keep `ui.rs` and `ui_viewport.rs` on the `TuiState` supertrait throughout.

Each step is behavior-preserving (data accessors only) and compiles
independently, so it can be merged between other agents' work without a
big-bang conflict.

## Verification

- `cargo check -p jcode-tui` after each sub-trait extraction (TMPDIR must point
  at real disk, not the RAM-backed tmpfs, or ring/aws-lc-sys build scripts fail
  with "Disk quota exceeded").
- `cargo test -p jcode-tui --lib` once at the end. Note: the lib test suite has
  pre-existing flaky parallel-order failures unrelated to this trait (verify any
  failing test in isolation with `--test-threads=1`).
