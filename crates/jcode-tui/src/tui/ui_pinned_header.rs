use super::*;

/// A successful content read is not evidence that the document's claims are current.
/// Only an explicit refresh failure or recovery state gets a warning.
pub(super) fn freshness_label(
    page: &crate::side_panel::SidePanelPage,
    unavailable: Option<&str>,
    now_ms: u64,
) -> String {
    if page.status.stale_reason.is_some() {
        return if page.status.last_successful_update_ms > 0 {
            "Source unavailable · showing saved copy".into()
        } else {
            "Source unavailable".into()
        };
    }
    if unavailable.is_some() {
        return "Snapshot · refresh unavailable".into();
    }
    if page.status.restored_from_session.is_some() {
        return "Snapshot · restored after recovery".into();
    }
    let updated = page.status.last_successful_update_ms;
    if updated == 0 || updated > now_ms {
        return String::new();
    }
    let seconds = (now_ms - updated) / 1000;
    let age = if seconds < 60 {
        "just now".into()
    } else if seconds < 3600 {
        format!("{}m ago", seconds / 60)
    } else if seconds < 86400 {
        format!("{}h ago", seconds / 3600)
    } else {
        format!("{}d ago", seconds / 86400)
    };
    format!("Updated {age}")
}

pub(super) fn panel_header(
    title: &str,
    badge: &str,
    page_index: usize,
    page_count: usize,
    fullscreen: bool,
    focused: bool,
    scrollable_images: bool,
    zoom: u8,
    width: usize,
) -> Line<'static> {
    let mut parts = vec![Span::styled(" side ", Style::default().fg(tool_color()))];
    // Put refresh warnings before the arbitrary document title so they survive
    // truncation on narrow terminals.
    if !badge.is_empty() {
        parts.push(Span::styled(
            format!("{badge} · "),
            Style::default().fg(dim_color()),
        ));
    }
    parts.push(Span::styled(
        title.replace(['\n', '\r', '\t'], " "),
        Style::default()
            .fg(rgb(180, 200, 255))
            .add_modifier(ratatui::style::Modifier::BOLD),
    ));
    parts.push(Span::styled(
        format!(" {page_index}/{page_count} "),
        Style::default().fg(dim_color()),
    ));
    parts.push(Span::styled(
        format!(
            " {} {} ",
            crate::tui::keybind::side_panel_toggle_key_label(),
            if fullscreen { "hide" } else { "fullscreen" }
        ),
        Style::default().fg(dim_color()),
    ));
    if focused {
        parts.push(Span::styled(
            " j/k scroll ",
            Style::default().fg(dim_color()),
        ));
        if page_count > 1 {
            parts.push(Span::styled(
                " Tab/Shift-Tab pages ",
                Style::default().fg(dim_color()),
            ));
        }
        parts.push(Span::styled(
            " Esc focus chat ",
            Style::default().fg(dim_color()),
        ));
    }
    if scrollable_images {
        parts.push(Span::styled(
            " readable scroll ",
            Style::default().fg(accent_color()),
        ));
        if focused {
            parts.push(Span::styled(
                " h/l pan +/- zoom ",
                Style::default().fg(dim_color()),
            ));
        }
    }
    if zoom != 100 {
        parts.push(Span::styled(
            format!(" zoom {zoom}% "),
            Style::default().fg(accent_color()),
        ));
    }
    super::truncate_line_with_ellipsis_to_width(&Line::from(parts), width)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn freshness_uses_real_success_time_not_legacy_revision_hash() {
        let mut page = crate::side_panel::SidePanelPage::default();
        page.updated_at_ms = u64::MAX;
        assert_eq!(freshness_label(&page, None, 120_000), "");
        page.status.last_successful_update_ms = 60_000;
        assert_eq!(freshness_label(&page, None, 120_000), "Updated 1m ago");
        assert!(!freshness_label(&page, None, 999_999_999).contains("stale"));
        page.status.stale_reason = Some("missing source".into());
        assert_eq!(
            freshness_label(&page, None, 120_000),
            "Source unavailable · showing saved copy"
        );
    }

    #[test]
    fn snapshot_badges_explain_retention() {
        let mut page = crate::side_panel::SidePanelPage::default();
        assert_eq!(
            freshness_label(&page, Some("read failed"), 0),
            "Snapshot · refresh unavailable"
        );
        page.status.restored_from_session = Some("old".into());
        assert_eq!(
            freshness_label(&page, None, 0),
            "Snapshot · restored after recovery"
        );
    }

    #[test]
    fn panel_header_never_wraps_unicode_or_multiline_titles() {
        for width in 0..100 {
            let line = panel_header(
                "界界👩‍💻 title\nnext",
                "Source unavailable · showing saved copy",
                1,
                2,
                false,
                true,
                true,
                120,
                width,
            );
            assert!(line.width() <= width, "width {width}: {line:?}");
            assert!(
                line.spans
                    .iter()
                    .all(|s| !s.content.contains(['\n', '\r', '\t']))
            );
        }
    }
}
