use super::*;

pub(super) use jcode_tui_messages::{centered_wrap_width, left_pad_lines_for_centered_mode};

pub(crate) fn get_cached_message_lines<F>(
    msg: &DisplayMessage,
    width: u16,
    diff_mode: crate::config::DiffDisplayMode,
    render: F,
) -> Vec<Line<'static>>
where
    F: FnOnce(&DisplayMessage, u16, crate::config::DiffDisplayMode) -> Vec<Line<'static>>,
{
    jcode_tui_messages::get_cached_message_lines(
        msg,
        width,
        diff_mode,
        jcode_tui_messages::MessageCacheContext {
            diagram_mode: crate::config::config().display.diagram_mode,
            centered: markdown::center_code_blocks(),
            mermaid_epoch: crate::tui::mermaid::deferred_render_epoch(),
            mermaid_aspect_bucket: crate::tui::mermaid::current_preferred_aspect_ratio_bucket(),
        },
        render,
    )
}
