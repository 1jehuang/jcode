use super::*;

fn provider_card_message() -> DisplayMessage {
    let mut msg = DisplayMessage::usage(
        "*%\t91\n= 5-hour window\t34%\t2h 6m\n= 7-day Opus window\t91%\t4d 2h\n\n+ Session resets\t1 available\n: Account\tje***@gmail.com\n- Run `/usage claude` for this account only."
            .to_string(),
    );
    msg.title = Some("Anthropic (Claude)".into());
    msg
}

#[test]
fn provider_card_never_overflows_and_keeps_a_straight_frame() {
    let msg = provider_card_message();
    for width in 0..=160u16 {
        let lines = render_usage_message(&msg, width, crate::config::DiffDisplayMode::Off);
        assert_eq!(lines.is_empty(), width == 0, "width {width}");
        for line in &lines {
            assert!(
                line.width() <= usize::from(width),
                "width {width}: {line:?}"
            );
        }
        let text: Vec<_> = lines.iter().map(extract_line_text).collect();
        if text.first().is_some_and(|row| row.starts_with('╭')) {
            assert!(
                lines.iter().all(|line| line.width() == lines[0].width()),
                "width {width}: {text:#?}"
            );
            assert!(text.last().unwrap().starts_with('╰'), "width {width}");
        }
    }
    // Wide terminals cap the card instead of stretching it.
    let render = |width| render_usage_message(&msg, width, crate::config::DiffDisplayMode::Off);
    assert_eq!(render(100), render(200));
    assert_eq!(render(200)[0].width(), 64);
}

#[test]
fn provider_card_aligns_gauge_and_detail_columns() {
    let msg = provider_card_message();
    let lines = render_usage_message(&msg, 100, crate::config::DiffDisplayMode::Off);
    let text: Vec<_> = lines.iter().map(extract_line_text).collect();
    assert!(text[0].contains("Anthropic (Claude)"), "{text:#?}");
    let gauges: Vec<_> = text.iter().filter(|row| row.contains('▰')).collect();
    assert_eq!(gauges.len(), 2, "{text:#?}");
    let column = |row: &str, needle: &str| row[..row.find(needle).unwrap()].width();
    // Bars, percentages and countdowns each start in one column.
    assert_eq!(column(gauges[0], "▰"), column(gauges[1], "▰"));
    assert_eq!(column(gauges[0], "%"), column(gauges[1], "%"));
    assert_eq!(column(gauges[0], "↻"), column(gauges[1], "↻"));
    // Detail values line up with the bars.
    let account = text
        .iter()
        .find(|row| row.contains("je***@gmail.com"))
        .unwrap();
    assert_eq!(column(account, "je***"), column(gauges[0], "▰"));
    // Notes drop the backticks and keep the command readable.
    assert!(
        text.iter().any(|row| row.contains("Run /usage claude")),
        "{text:#?}"
    );
    assert!(text.iter().all(|row| !row.contains('`')));
}

#[test]
fn provider_card_stacks_labels_instead_of_clipping_when_narrow() {
    let msg = provider_card_message();
    let lines = render_usage_message(&msg, 38, crate::config::DiffDisplayMode::Off);
    let text: Vec<_> = lines.iter().map(extract_line_text).collect();
    // Brand stays in the border, the dim qualifier gives way to the status.
    assert!(
        text[0].contains("Anthropic") && text[0].contains('●'),
        "{text:#?}"
    );
    for expected in [
        "5-hour window",
        "2h 6m",
        "4d 2h",
        "Session resets",
        "1 available",
    ] {
        assert!(
            text.iter().any(|row| row.contains(expected)),
            "missing {expected}: {text:#?}"
        );
    }
    let gauges = text.iter().filter(|row| row.contains('▰')).count();
    assert_eq!(gauges, 2, "{text:#?}");
    // Every framed width keeps whole gauge labels and countdowns: they wrap,
    // they are never clipped with an ellipsis.
    for width in 22..=80u16 {
        let lines = render_usage_message(&msg, width, crate::config::DiffDisplayMode::Off);
        let body: String = lines[1..]
            .iter()
            .map(extract_line_text)
            .collect::<Vec<_>>()
            .join(" ");
        let body = without_whitespace(&body).replace('│', "");
        for expected in ["5-hourwindow", "7-dayOpuswindow", "↻2h6m", "↻4d2h"] {
            assert!(
                body.contains(expected),
                "width {width}: missing {expected}: {body}"
            );
        }
        assert!(!body.contains('…'), "width {width}: {body}");
        // The status pill stays visible, in the border or as the first row.
        let header: String = lines[..2].iter().map(extract_line_text).collect();
        assert!(header.contains('●'), "width {width}: {header}");
    }
}

#[test]
fn provider_card_status_uses_shared_limit_colors() {
    use crate::tui::info_widget::usage_limit_color;
    let display_used = crate::config::config().display.usage_display_used();
    let status_span = |content: &str, title: &str| {
        let mut msg = DisplayMessage::usage(content.to_string());
        msg.title = Some(title.into());
        render_usage_message(&msg, 100, crate::config::DiffDisplayMode::Off)[0]
            .spans
            .iter()
            .find(|span| span.content.starts_with('●') || span.content.starts_with('◌'))
            .map(|span| span.style.fg)
            .unwrap()
    };
    assert_eq!(status_span("*%\t91", "A"), Some(usage_limit_color(9)));
    assert_eq!(status_span("*%\t10", "A"), Some(usage_limit_color(90)));
    assert_eq!(
        status_span("*crit\terror\n! boom", "A"),
        Some(usage_limit_color(0))
    );
    let wording = render_usage_message(
        &provider_card_message(),
        100,
        crate::config::DiffDisplayMode::Off,
    );
    let header = extract_line_text(&wording[0]);
    if display_used {
        assert!(header.contains("91% used"), "{header}");
    } else {
        assert!(header.contains("9% left"), "{header}");
    }
}
