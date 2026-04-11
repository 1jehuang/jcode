use super::{
    accent_color, ai_color, ai_text, asap_color, clear_area, dim_color, get_grouped_changelog,
    header_icon_color, header_name_color, header_session_color, pending_color, queued_color, rgb,
    tool_color, user_bg, user_color, user_text,
};
use crate::tui::TuiState;
use crate::tui::info_widget::WidgetPlacement;
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Paragraph},
};

pub(super) fn draw_changelog_overlay(frame: &mut Frame, area: Rect, scroll: usize) {
    clear_area(frame, area);

    let groups = get_grouped_changelog();
    let mut lines: Vec<Line<'static>> = Vec::new();

    if groups.is_empty() {
        lines.push(Line::from(Span::styled(
            "No changelog entries available.",
            Style::default().fg(dim_color()),
        )));
    } else {
        for group in &groups {
            let heading = match &group.released_at {
                Some(released_at) => format!("  {} · {}", group.version, released_at),
                None => format!("  {}", group.version),
            };
            lines.push(Line::from(Span::styled(
                heading,
                Style::default()
                    .fg(rgb(200, 200, 220))
                    .add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(""));
            for entry in &group.entries {
                lines.push(Line::from(vec![
                    Span::styled("    • ", Style::default().fg(dim_color())),
                    Span::styled(entry.clone(), Style::default().fg(rgb(170, 170, 185))),
                ]));
            }
            lines.push(Line::from(""));
        }
    }

    let total_lines = lines.len();
    let visible_height = area.height.saturating_sub(2) as usize;
    let max_scroll = total_lines.saturating_sub(visible_height);
    let scroll = scroll.min(max_scroll);

    let scroll_info = if total_lines > visible_height {
        let pct = if max_scroll > 0 {
            (scroll * 100) / max_scroll
        } else {
            100
        };
        format!(" {}% ", pct)
    } else {
        String::new()
    };

    let title = format!(" Changelog {} ", scroll_info);
    let block = Block::default()
        .title(Span::styled(
            title,
            Style::default()
                .fg(rgb(200, 200, 220))
                .add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Line::from(Span::styled(
            " Esc to close · mouse wheel/j/k scroll · Space/PageUp page ",
            Style::default().fg(dim_color()),
        )))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(dim_color()));

    let paragraph = Paragraph::new(lines)
        .block(block)
        .scroll((scroll as u16, 0));

    frame.render_widget(paragraph, area);
}

pub(super) fn draw_help_overlay(frame: &mut Frame, area: Rect, scroll: usize, app: &dyn TuiState) {
    clear_area(frame, area);

    let section_style = Style::default()
        .fg(accent_color())
        .add_modifier(Modifier::BOLD);
    let cmd_style = Style::default().fg(rgb(230, 230, 240));
    let desc_style = Style::default().fg(rgb(150, 150, 165));
    let key_style = Style::default().fg(rgb(200, 180, 120));
    let sep_style = Style::default().fg(rgb(50, 50, 55));

    let mut lines: Vec<Line<'static>> = Vec::new();

    let separator = || -> Line<'static> {
        Line::from(Span::styled(
            "  ─────────────────────────────────────────────────",
            sep_style,
        ))
    };

    let help_entry = |cmd: &str, desc: &str| -> Line<'static> {
        Line::from(vec![
            Span::styled("    ", Style::default()),
            Span::styled(cmd.to_string(), cmd_style),
            Span::styled("  ", Style::default()),
            Span::styled(desc.to_string(), desc_style),
        ])
    };

    let key_entry = |key: &str, desc: &str| -> Line<'static> {
        Line::from(vec![
            Span::styled("    ", Style::default()),
            Span::styled(format!("{:<22}", key), key_style),
            Span::styled(desc.to_string(), desc_style),
        ])
    };

    lines.push(Line::from(""));

    lines.push(Line::from(Span::styled("  Commands", section_style)));
    lines.push(Line::from(""));
    lines.push(help_entry("/help", "Show this help overlay"));
    lines.push(help_entry(
        "/help <command>",
        "Show details for one command",
    ));
    lines.push(help_entry("/model", "List or switch models"));
    lines.push(help_entry("/model <name>", "Switch to a different model"));
    lines.push(help_entry("/agents", "Configure models for agent roles"));
    lines.push(help_entry(
        "/effort <level>",
        "Set reasoning effort (none|low|medium|high|xhigh)",
    ));
    lines.push(help_entry(
        "/fast [on|off|status|default ...]",
        "Toggle OpenAI/Codex fast mode",
    ));
    lines.push(help_entry(
        "/transport <mode>",
        "Set connection transport (auto|https|websocket)",
    ));
    lines.push(help_entry(
        "/alignment [status|centered|left]",
        "Show or persist text alignment preference",
    ));
    lines.push(help_entry("/config", "Show active configuration"));
    lines.push(help_entry("/config init", "Create default config file"));
    lines.push(help_entry("/config edit", "Open config in $EDITOR"));
    lines.push(help_entry("/dictate", "Run configured external dictation"));
    lines.push(help_entry(
        "/git [status]",
        "Show branch and working tree status for the repo",
    ));
    lines.push(help_entry(
        "/context",
        "Show the full session context snapshot",
    ));
    lines.push(help_entry("/info", "Show session info and token usage"));
    lines.push(help_entry("/usage", "Show connected provider usage limits"));
    lines.push(help_entry("/version", "Show version and build details"));
    lines.push(help_entry(
        "/changelog",
        "Show recent changes in this build",
    ));

    lines.push(Line::from(""));
    lines.push(separator());
    lines.push(Line::from(""));

    lines.push(Line::from(Span::styled("  Session", section_style)));
    lines.push(Line::from(""));
    lines.push(help_entry("/clear", "Clear conversation and start fresh"));
    lines.push(help_entry(
        "/compact",
        "Summarize old messages to free context",
    ));
    lines.push(help_entry(
        "/rewind",
        "Show numbered history, /rewind N to rewind",
    ));
    lines.push(help_entry(
        "/fix",
        "Attempt recovery when model cannot continue",
    ));
    lines.push(help_entry(
        "/poke",
        "Remind model to finish incomplete todos",
    ));
    lines.push(help_entry(
        "/improve",
        "Autonomously improve the repo until returns diminish",
    ));
    lines.push(help_entry(
        "/improve resume",
        "Resume the last saved improve loop/plan",
    ));
    lines.push(help_entry(
        "/refactor",
        "Run a safe refactor loop with independent review",
    ));
    lines.push(help_entry(
        "/refactor resume",
        "Resume the last saved refactor loop/plan",
    ));
    lines.push(help_entry("/split", "Clone session into a new window"));
    lines.push(help_entry(
        "/workspace [status|on|off|add]",
        "Enable and manage the Niri-style session workspace",
    ));
    lines.push(help_entry(
        "/catchup [next|list]",
        "Jump to finished sessions and open a Catch Up brief",
    ));
    lines.push(help_entry(
        "/back",
        "Return to the previous Catch Up source session",
    ));
    lines.push(help_entry("/resume", "Browse and resume previous sessions"));
    lines.push(help_entry(
        "/catchup [next]",
        "Jump into finished sessions with a side-panel brief",
    ));
    lines.push(help_entry(
        "/back",
        "Return to the previous Catch Up session",
    ));
    lines.push(help_entry("/save [label]", "Bookmark session for /resume"));
    lines.push(help_entry(
        "/unsave",
        "Remove bookmark from current session",
    ));

    lines.push(Line::from(""));
    lines.push(separator());
    lines.push(Line::from(""));

    lines.push(Line::from(Span::styled("  Memory & Swarm", section_style)));
    lines.push(Line::from(""));
    lines.push(help_entry("/memory [on|off]", "Toggle memory features"));
    lines.push(help_entry("/goals", "Open goals overview / resume a goal"));
    lines.push(help_entry("/swarm [on|off]", "Toggle swarm features"));

    lines.push(Line::from(""));
    lines.push(separator());
    lines.push(Line::from(""));

    lines.push(Line::from(Span::styled("  Auth & Accounts", section_style)));
    lines.push(Line::from(""));
    lines.push(help_entry("/auth", "Show authentication status"));
    lines.push(help_entry(
        "/login [provider]",
        "Interactive or direct login",
    ));
    lines.push(help_entry(
        "/account",
        "Open combined Claude/OpenAI account picker",
    ));
    lines.push(help_entry(
        "/subscription",
        "Inspect jcode subscription scaffold",
    ));

    lines.push(Line::from(""));
    lines.push(separator());
    lines.push(Line::from(""));

    lines.push(Line::from(Span::styled("  System", section_style)));
    lines.push(Line::from(""));
    lines.push(help_entry("/reload", "Reload to newer binary if available"));
    lines.push(help_entry(
        "/restart",
        "Restart with current binary (no build)",
    ));
    lines.push(help_entry(
        "/rebuild",
        "Full update (git pull + build + tests)",
    ));
    if app.is_remote_mode() {
        lines.push(help_entry("/client-reload", "Force reload client binary"));
        lines.push(help_entry("/server-reload", "Force reload server binary"));
    }
    lines.push(help_entry(
        "/debug-visual",
        "Enable visual debugging for TUI issues",
    ));
    lines.push(help_entry("/quit", "Exit jcode"));

    let skills = app.available_skills();
    if !skills.is_empty() {
        lines.push(Line::from(""));
        lines.push(separator());
        lines.push(Line::from(""));

        lines.push(Line::from(Span::styled("  Skills", section_style)));
        lines.push(Line::from(""));
        for skill in &skills {
            lines.push(help_entry(&format!("/{}", skill), "Activate skill"));
        }
    }

    lines.push(Line::from(""));
    lines.push(separator());
    lines.push(Line::from(""));

    lines.push(Line::from(Span::styled("  Navigation", section_style)));
    lines.push(Line::from(""));
    lines.push(key_entry("PageUp / PageDown", "Scroll history"));
    lines.push(key_entry("Up / Down", "Scroll history (when input empty)"));
    lines.push(key_entry("Ctrl+[ / Ctrl+]", "Jump between user prompts"));
    lines.push(key_entry("Ctrl+1..4", "Resize side panel to 25/50/75/100%"));
    lines.push(key_entry(
        "Ctrl+5..9",
        "Jump by recency (5 = 5th most recent)",
    ));

    lines.push(Line::from(""));
    lines.push(separator());
    lines.push(Line::from(""));

    lines.push(Line::from(Span::styled(
        "  Diagrams & Diffs",
        section_style,
    )));
    lines.push(Line::from(""));
    lines.push(key_entry(
        "Alt+M",
        "Toggle side panel (or diagram pane if empty)",
    ));
    lines.push(key_entry("Alt+T", "Toggle diagram position (side/top)"));
    lines.push(key_entry("Ctrl+H / Ctrl+L", "Focus chat / diagram / diffs"));
    lines.push(key_entry(
        "Ctrl+Left / Right",
        "Cycle diagrams (when diagram focused)",
    ));
    lines.push(key_entry("h/j/k/l / arrows", "Pan diagram (when focused)"));
    lines.push(key_entry("[ / ]", "Zoom diagram (when focused)"));
    lines.push(key_entry("+ / -", "Resize diagram pane"));
    lines.push(key_entry(
        "Shift+Tab",
        "Cycle diff mode (Off/Inline/Pinned)",
    ));

    lines.push(Line::from(""));
    lines.push(separator());
    lines.push(Line::from(""));

    lines.push(Line::from(Span::styled("  Input & Editing", section_style)));
    lines.push(Line::from(""));
    lines.push(key_entry(
        "Ctrl+C / Ctrl+D",
        "Quit (press twice to confirm)",
    ));
    lines.push(key_entry("Ctrl+X", "Cut entire input line to clipboard"));
    lines.push(key_entry(
        "Ctrl+A",
        "Copy visible chat viewport plus nearby context",
    ));
    lines.push(key_entry("Ctrl+U", "Clear input line"));
    lines.push(key_entry("Ctrl+S", "Stash / pop input (save for later)"));
    lines.push(key_entry("Ctrl+Backspace", "Delete previous word in input"));
    lines.push(key_entry("Ctrl+B / Ctrl+F", "Move by word left / right"));
    lines.push(key_entry("Ctrl+Left / Right", "Move by word left / right"));
    lines.push(key_entry("Shift+Enter", "Insert newline in input"));
    lines.push(key_entry(
        "Ctrl+Enter",
        "Use opposite send mode while processing",
    ));
    lines.push(key_entry("Ctrl+Up", "Retrieve pending message for editing"));
    lines.push(key_entry("Ctrl+Tab / Ctrl+T", "Toggle queue mode"));
    lines.push(key_entry("Ctrl+R", "Recover from missing tool outputs"));
    lines.push(key_entry("Ctrl+V", "Paste clipboard (text or image)"));
    lines.push(key_entry("Alt+V", "Paste image from clipboard"));
    lines.push(key_entry(
        "Alt+A",
        "Quick-copy visible chat viewport plus nearby context",
    ));
    lines.push(key_entry("Alt+Y", "Toggle chat selection/copy mode"));
    lines.push(key_entry("Alt+S", "Toggle typing scroll lock"));
    lines.push(key_entry("Alt+Left / Right", "Cycle reasoning effort"));
    if let Some(label) = app.dictation_key_label() {
        lines.push(key_entry(&label, "Run configured dictation"));
    }

    lines.push(Line::from(""));

    let total_lines = lines.len();
    let visible_height = area.height.saturating_sub(2) as usize;
    let max_scroll = total_lines.saturating_sub(visible_height);
    let scroll = scroll.min(max_scroll);

    let scroll_info = if total_lines > visible_height {
        let pct = if max_scroll > 0 {
            (scroll * 100) / max_scroll
        } else {
            100
        };
        format!(" {}% ", pct)
    } else {
        String::new()
    };

    let title = format!(" Help {} ", scroll_info);
    let block = Block::default()
        .title(Span::styled(
            title,
            Style::default()
                .fg(rgb(200, 200, 220))
                .add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Line::from(Span::styled(
            " Esc to close · mouse wheel/j/k scroll · Space/PageUp page · /help <cmd> for details ",
            Style::default().fg(dim_color()),
        )))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(dim_color()));

    let paragraph = Paragraph::new(lines)
        .block(block)
        .scroll((scroll as u16, 0));

    frame.render_widget(paragraph, area);
}

pub(super) fn draw_debug_overlay(
    frame: &mut Frame,
    placements: &[WidgetPlacement],
    chunks: &[Rect],
) {
    if chunks.len() < 5 {
        return;
    }
    render_overlay_box(frame, chunks[0], "messages", Color::Red);
    render_overlay_box(frame, chunks[1], "queued", Color::Yellow);
    render_overlay_box(frame, chunks[2], "status", Color::Cyan);
    render_overlay_box(frame, chunks[3], "picker", Color::Magenta);
    render_overlay_box(frame, chunks[4], "input", Color::Green);
    if chunks.len() > 5 && chunks[5].height > 0 {
        render_overlay_box(frame, chunks[5], "donut", Color::Blue);
    }

    for placement in placements {
        let title = format!("widget:{}", placement.kind.as_str());
        render_overlay_box(frame, placement.rect, &title, Color::Magenta);
    }
}

fn render_overlay_box(frame: &mut Frame, area: Rect, title: &str, color: Color) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(color))
        .title(Span::styled(title.to_string(), Style::default().fg(color)));
    frame.render_widget(block, area);
}

pub(super) fn debug_palette_json() -> Option<serde_json::Value> {
    Some(serde_json::json!({
        "user_color": color_to_rgb(user_color()),
        "ai_color": color_to_rgb(ai_color()),
        "tool_color": color_to_rgb(tool_color()),
        "dim_color": color_to_rgb(dim_color()),
        "accent_color": color_to_rgb(accent_color()),
        "queued_color": color_to_rgb(queued_color()),
        "asap_color": color_to_rgb(asap_color()),
        "pending_color": color_to_rgb(pending_color()),
        "user_text": color_to_rgb(user_text()),
        "user_bg": color_to_rgb(user_bg()),
        "ai_text": color_to_rgb(ai_text()),
        "header_icon_color": color_to_rgb(header_icon_color()),
        "header_name_color": color_to_rgb(header_name_color()),
        "header_session_color": color_to_rgb(header_session_color()),
    }))
}

fn color_to_rgb(color: Color) -> Option<[u8; 3]> {
    match color {
        Color::Rgb(r, g, b) => Some([r, g, b]),
        Color::Indexed(n) if n >= 16 => {
            let (r, g, b) = crate::tui::color_support::indexed_to_rgb(n);
            Some([r, g, b])
        }
        _ => None,
    }
}
