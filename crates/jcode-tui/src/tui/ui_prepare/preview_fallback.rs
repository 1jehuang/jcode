//! Recoverable placeholder for an unexpectedly incomplete unbounded preview.
use super::*;

pub(super) fn unavailable(width: u16) -> PreparedMessages {
    crate::logging::warn("Unbounded history preview unexpectedly failed to complete");
    wrap_lines_with_map(
        vec![Line::raw("Preview unavailable")],
        &[],
        &[],
        &[],
        &[],
        &[],
        width,
        &[],
        &[],
        &[],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unavailable_preview_is_visible_instead_of_panicking() {
        let preview = unavailable(80);
        assert!(
            preview
                .wrapped_plain_lines
                .iter()
                .any(|line| line.contains("Preview unavailable"))
        );
        assert_eq!(&*preview.raw_plain_lines, &["Preview unavailable"]);
    }
}
