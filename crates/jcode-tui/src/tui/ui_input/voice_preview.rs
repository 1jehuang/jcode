use super::*;

pub(super) fn voice_ghost(
    preview: Option<&crate::tui::VoiceComposerPreview>,
) -> Option<(&str, Style)> {
    let preview = preview?;
    let color = if preview.recording {
        rgb(200, 200, 210)
    } else {
        rgb(140, 200, 255)
    };
    Some((
        preview.live.as_str(),
        Style::default().fg(dim_color_of(color)).italic(),
    ))
}

fn dim_color_of(color: Color) -> Color {
    match color {
        Color::Rgb(r, g, b) => rgb(
            (r as u16 * 3 / 4) as u8,
            (g as u16 * 3 / 4) as u8,
            (b as u16 * 3 / 4) as u8,
        ),
        other => other,
    }
}

/// Truncate to `width` terminal cells, ending in `…` when cut.
pub(super) fn truncate_cells(text: &str, width: usize) -> String {
    use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
    if text.width() <= width {
        return text.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let w = ch.width().unwrap_or(0);
        if used + w + 1 > width {
            break;
        }
        used += w;
        out.push(ch);
    }
    if width > 0 {
        out.push('…');
    }
    out
}

/// Smooth level to a 0..=1 bar height. Speech RMS sits around 0.02 to 0.2,
/// so the curve is compressed to keep normal speech visibly moving.
fn voice_level_height(level: f32) -> f32 {
    (level.max(0.0) * 4.0).sqrt().min(1.0)
}

/// Header row above the composer while voice input is active:
/// `● 0:07 ▁▂▅▇▆▃▂▁▂▄… · Ctrl+Space send · Esc cancel`. The waveform fills the
/// width left after the label and hint, newest sample on the right, colored
/// by loudness. Never wraps: the hint is dropped first, then the waveform
/// shrinks.
pub(super) fn voice_waveform_line(
    preview: &crate::tui::VoiceComposerPreview,
    width: usize,
) -> Line<'static> {
    use unicode_width::UnicodeWidthStr;
    const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let mut spans: Vec<Span<'static>> = Vec::new();
    if !preview.recording {
        let dots = ["◌", "◍", "●", "◍"];
        let phase = (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() / 250)
            .unwrap_or(0)
            % dots.len() as u128) as usize;
        let mut text = format!("  {} Transcribing…", dots[phase]);
        if !preview.hint.is_empty() {
            text.push_str(" · ");
            text.push_str(&preview.hint);
        }
        let text = truncate_cells(&text, width);
        spans.push(Span::styled(
            text,
            Style::default().fg(rgb(140, 200, 255)).bold(),
        ));
        return Line::from(spans);
    }
    let label = format!(
        "  ● {}:{:02} ",
        preview.elapsed_secs / 60,
        preview.elapsed_secs % 60
    );
    let hint = if preview.hint.is_empty() {
        String::new()
    } else {
        format!("  {}", preview.hint)
    };
    let label_w = label.width();
    let mut hint_w = hint.width();
    if label_w + hint_w + 8 > width {
        hint_w = 0;
    }
    let wave_w = width.saturating_sub(label_w + hint_w).min(48);
    spans.push(Span::styled(
        truncate_cells(&label, width),
        Style::default().fg(rgb(255, 110, 110)).bold(),
    ));
    if wave_w > 0 {
        let levels = &preview.levels;
        let start = levels.len().saturating_sub(wave_w);
        let pad = wave_w.saturating_sub(levels.len() - start);
        if pad > 0 {
            spans.push(Span::styled(
                "·".repeat(pad),
                Style::default().fg(rgb(70, 70, 80)),
            ));
        }
        for level in &levels[start..] {
            let h = voice_level_height(*level);
            let glyph = BARS[(h * 7.0).round() as usize];
            // Quiet bars recede, loud bars glow warm.
            let color = rgb(
                (110.0 + 145.0 * h) as u8,
                (150.0 - 40.0 * h) as u8,
                (190.0 - 80.0 * h) as u8,
            );
            spans.push(Span::styled(glyph.to_string(), Style::default().fg(color)));
        }
    }
    if hint_w > 0 {
        spans.push(Span::styled(hint, Style::default().fg(dim_color())));
    }
    Line::from(spans)
}
