use ratatui::{prelude::*, widgets::Paragraph};
use std::sync::atomic::{AtomicUsize, Ordering};

use super::layout_support::clear_area;
use super::theme_support::dim_color;

/// Number of recovered panics while rendering the frame.
static DRAW_PANIC_COUNT: AtomicUsize = AtomicUsize::new(0);

pub(super) fn render_recovered_panic_frame(
    frame: &mut Frame,
    payload: &(dyn std::any::Any + Send),
) {
    let panic_count = DRAW_PANIC_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    let msg = panic_payload_to_string(payload);
    if panic_count <= 3 || panic_count.is_multiple_of(50) {
        crate::logging::error(&format!(
            "Recovered TUI draw panic #{}: {}",
            panic_count, msg
        ));
    }
    let area = frame.area().intersection(*frame.buffer_mut().area());
    if area.width == 0 || area.height == 0 {
        return;
    }
    clear_area(frame, area);
    let lines = vec![
        Line::from(Span::styled(
            "rendering error recovered",
            Style::default().fg(Color::Red),
        )),
        Line::from(Span::styled(
            "continuing with a safe fallback frame",
            Style::default().fg(dim_color()),
        )),
    ];
    frame.render_widget(Paragraph::new(lines), area);
}

fn panic_payload_to_string(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic payload".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::panic_payload_to_string;

    #[test]
    fn panic_payload_to_string_handles_common_payloads() {
        let str_payload: &(dyn std::any::Any + Send) = &"borrowed panic";
        let string_payload: &(dyn std::any::Any + Send) = &String::from("owned panic");
        let unknown_payload: &(dyn std::any::Any + Send) = &42usize;

        assert_eq!(panic_payload_to_string(str_payload), "borrowed panic");
        assert_eq!(panic_payload_to_string(string_payload), "owned panic");
        assert_eq!(
            panic_payload_to_string(unknown_payload),
            "unknown panic payload"
        );
    }
}
