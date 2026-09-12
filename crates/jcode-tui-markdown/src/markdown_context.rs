use crate::{DiagramDisplayMode, MarkdownSpacingMode};
use std::cell::Cell;
use std::sync::{LazyLock, Mutex};

static DIAGRAM_MODE_OVERRIDE: LazyLock<Mutex<Option<DiagramDisplayMode>>> =
    LazyLock::new(|| Mutex::new(None));

thread_local! {
    /// Whether markdown rendering is running in streaming mode.
    /// In this mode mermaid diagrams update an ephemeral side-panel preview
    /// instead of being persisted in ACTIVE_DIAGRAMS history.
    static STREAMING_RENDER_CONTEXT: Cell<bool> = const { Cell::new(false) };
    /// Whether code blocks should be horizontally centered within available width.
    /// Set to true in centered mode, false in left-aligned mode.
    static CENTER_CODE_BLOCKS: Cell<bool> = const { Cell::new(true) };
    /// Optional test/debug override for markdown spacing mode.
    static MARKDOWN_SPACING_MODE_OVERRIDE: Cell<Option<MarkdownSpacingMode>> = const { Cell::new(None) };
    /// Whether Mermaid cache misses should be rendered in the background and
    /// replaced on a later redraw instead of blocking the current frame.
    static DEFER_MERMAID_RENDER_CONTEXT: Cell<bool> = const { Cell::new(false) };
    /// Optional test/debug override for whether mermaid rendering is enabled.
    /// Thread-local (not process-global) so tests that disable mermaid cannot
    /// race other test threads that rely on the process-env default.
    static MERMAID_RENDERING_OVERRIDE: Cell<Option<bool>> = const { Cell::new(None) };
    /// Scoped, thread-local diagram display mode. Takes precedence over the
    /// process-global override so render paths (e.g. the side panel, which
    /// renders with diagrams inline) can pin a mode for one render without
    /// mutating global state that concurrent threads observe.
    static DIAGRAM_MODE_SCOPE: Cell<Option<DiagramDisplayMode>> = const { Cell::new(None) };
}

struct ScopedReset<'a, T: Copy> {
    cell: &'a Cell<T>,
    prev: T,
}

impl<T: Copy> Drop for ScopedReset<'_, T> {
    fn drop(&mut self) {
        self.cell.set(self.prev);
    }
}

fn with_scoped_cell_value<T: Copy, R>(cell: &Cell<T>, value: T, f: impl FnOnce() -> R) -> R {
    let prev = cell.replace(value);
    let _guard = ScopedReset { cell, prev };
    f()
}

pub fn set_diagram_mode_override(mode: Option<DiagramDisplayMode>) {
    if let Ok(mut override_mode) = DIAGRAM_MODE_OVERRIDE.lock() {
        *override_mode = mode;
    }
}

pub fn get_diagram_mode_override() -> Option<DiagramDisplayMode> {
    DIAGRAM_MODE_OVERRIDE.lock().ok().and_then(|mode| *mode)
}

pub(super) fn effective_diagram_mode() -> DiagramDisplayMode {
    if let Some(scoped) = DIAGRAM_MODE_SCOPE.with(|ctx| ctx.get()) {
        return scoped;
    }
    if let Ok(mode) = DIAGRAM_MODE_OVERRIDE.lock()
        && let Some(override_mode) = *mode
    {
        return override_mode;
    }
    crate::config_snapshot().diagram_mode
}

/// Run `f` with the diagram display mode pinned on the current thread.
/// Takes precedence over both the process-global override and the config
/// snapshot, so a render path (e.g. the side panel, which always renders
/// diagrams inline) can pin a mode without mutating process-global state
/// that concurrent threads observe.
pub fn with_diagram_mode_scope<T>(mode: DiagramDisplayMode, f: impl FnOnce() -> T) -> T {
    DIAGRAM_MODE_SCOPE.with(|ctx| with_scoped_cell_value(ctx, Some(mode), f))
}

pub(super) fn effective_markdown_spacing_mode() -> MarkdownSpacingMode {
    MARKDOWN_SPACING_MODE_OVERRIDE.with(|mode| {
        mode.get()
            .unwrap_or(crate::config_snapshot().markdown_spacing)
    })
}

/// Whether mermaid diagram rendering is enabled.
///
/// The application supplies the effective config value, including environment
/// overrides, through [`crate::MarkdownConfigSnapshot`]. Tests should use
/// [`with_mermaid_rendering_override`] rather than mutating global config.
pub fn mermaid_rendering_enabled() -> bool {
    if let Some(enabled) = MERMAID_RENDERING_OVERRIDE.with(|ctx| ctx.get()) {
        return enabled;
    }
    crate::config_snapshot().mermaid_enabled
}

/// Run `f` with mermaid rendering forced on/off (or `None` to restore the
/// config-based default) on the current thread. Thread-local and scoped, so
/// parallel tests cannot observe each other's override.
pub fn with_mermaid_rendering_override<T>(enabled: Option<bool>, f: impl FnOnce() -> T) -> T {
    MERMAID_RENDERING_OVERRIDE.with(|ctx| with_scoped_cell_value(ctx, enabled, f))
}

#[cfg(test)]
pub(crate) fn with_markdown_spacing_mode_override<T>(
    mode: Option<MarkdownSpacingMode>,
    f: impl FnOnce() -> T,
) -> T {
    MARKDOWN_SPACING_MODE_OVERRIDE.with(|ctx| with_scoped_cell_value(ctx, mode, f))
}

pub(super) fn with_streaming_render_context<T>(f: impl FnOnce() -> T) -> T {
    STREAMING_RENDER_CONTEXT.with(|ctx| with_scoped_cell_value(ctx, true, f))
}

pub(super) fn streaming_render_context_enabled() -> bool {
    STREAMING_RENDER_CONTEXT.with(|ctx| ctx.get())
}

pub fn with_deferred_mermaid_render_context<T>(f: impl FnOnce() -> T) -> T {
    DEFER_MERMAID_RENDER_CONTEXT.with(|ctx| with_scoped_cell_value(ctx, true, f))
}

pub(super) fn deferred_mermaid_render_context_enabled() -> bool {
    DEFER_MERMAID_RENDER_CONTEXT.with(|ctx| ctx.get())
}

pub fn set_center_code_blocks(centered: bool) {
    CENTER_CODE_BLOCKS.with(|ctx| ctx.set(centered));
}

pub fn center_code_blocks() -> bool {
    CENTER_CODE_BLOCKS.with(|ctx| ctx.get())
}

/// Override markdown block alignment for one render, restoring the caller's
/// setting afterwards (including during unwinding).
pub fn with_center_code_blocks<T>(centered: bool, f: impl FnOnce() -> T) -> T {
    CENTER_CODE_BLOCKS.with(|ctx| with_scoped_cell_value(ctx, centered, f))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centered_scope_restores_nested_and_panicking_overrides() {
        let original = center_code_blocks();
        with_center_code_blocks(true, || {
            with_center_code_blocks(false, || assert!(!center_code_blocks()));
            assert!(center_code_blocks());
            let result = std::panic::catch_unwind(|| {
                with_center_code_blocks(false, || panic!("test unwind"));
            });
            assert!(result.is_err());
            assert!(center_code_blocks());
        });
        assert_eq!(center_code_blocks(), original);
    }
}
