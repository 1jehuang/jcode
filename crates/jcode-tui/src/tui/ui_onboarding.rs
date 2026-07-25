//! First-run onboarding welcome screen.
//!
//! Rendered in place of the normal empty-state transcript when
//! `TuiState::onboarding_welcome_active()` is true (brand-new install /
//! unauthenticated / new user, or `/onboarding-preview`).
//!
//! Layout, top to bottom, vertically centered in the chat area:
//!   1. Grayed telemetry notice header.
//!   2. The animated donut (attention grab).
//!   3. "Welcome to jcode onboarding" title.
//!   4. The login / getting-started prompt with suggestions.
//!
//! The donut is drawn as a live widget (not part of the cached transcript) so
//! it animates every frame, matching the idle-donut behavior elsewhere.

use super::animations;
use super::dim_color;
use crate::tui::TuiState;
use crate::tui::color_support::rgb;
use ratatui::{prelude::*, widgets::Paragraph};

const DONUT_HEIGHT: u16 = 18;
const TELEMETRY_LINES: u16 = 4;
const GAP: u16 = 1;

/// Accent color for the welcome title.
fn welcome_accent() -> Color {
    rgb(138, 180, 248)
}

/// Append the universal "Esc to skip" hint shown on every guided onboarding
/// phase. This advertises the escape hatch that guarantees the user can always
/// leave onboarding (see `handle_onboarding_continue_prompt_key`), so a first-
/// run user is never visibly trapped. Kept dim and on its own line so it never
/// competes with the primary action.
fn push_esc_skip_hint(lines: &mut Vec<Line<'static>>, align: Alignment) {
    lines.push(Line::from(""));
    lines.push(
        Line::from(Span::styled(
            "Esc to skip onboarding (log in later with /login).",
            Style::default().fg(dim_color()),
        ))
        .alignment(align),
    );
}

/// Whether the terminal renders the U+25D6/U+25D7 half-circle pill caps as
/// clean full-cell semicircles. Kitty does; ghostty, Apple Terminal, and the
/// VS Code terminal draw them as small floating glyphs that break the capsule
/// illusion, so those fall back to half-block caps that render solidly
/// everywhere. Override with `JCODE_ROUNDED_PILLS=on|off`.
fn rounded_pill_caps_supported() -> bool {
    static SUPPORTED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *SUPPORTED.get_or_init(|| {
        // Golden/eval tests assert the canonical rounded glyphs; keep test
        // renders deterministic regardless of the terminal running the tests.
        if cfg!(test) {
            return true;
        }
        if let Ok(raw) = std::env::var("JCODE_ROUNDED_PILLS") {
            match raw.trim().to_ascii_lowercase().as_str() {
                "1" | "true" | "yes" | "on" => return true,
                "0" | "false" | "no" | "off" => return false,
                _ => {}
            }
        }
        if let Ok(tp) = std::env::var("TERM_PROGRAM") {
            let tp = tp.to_ascii_lowercase();
            if tp == "ghostty" || tp == "apple_terminal" || tp == "vscode" {
                return false;
            }
        }
        if std::env::var("GHOSTTY_RESOURCES_DIR").is_ok()
            || std::env::var("GHOSTTY_BIN_DIR").is_ok()
        {
            return false;
        }
        if let Ok(term) = std::env::var("TERM")
            && term.to_ascii_lowercase().contains("ghostty")
        {
            return false;
        }
        true
    })
}

/// Build one rounded "lozenge" pill: half-circle end caps (`◖` / `◗`) around a
/// padded label. Both states are solid capsules; the selected pill has a bright
/// accent fill + BOLD label, the unselected one a muted dark-gray fill with no
/// bold. The BOLD-vs-not-bold contrast is a non-color attribute, so the
/// selection survives on monochrome terminals (Tier 10 color-independence).
fn lozenge_pill_spans(label: &str, filled: bool) -> Vec<Span<'static>> {
    // Both states are solid capsules (the ◖/◗ caps are filled half-circles, so a
    // "hollow" outline reads as stray half-moons). The selected pill uses the
    // bright accent fill + BOLD label; the unselected one a muted dark-gray fill
    // with no bold. The BOLD-vs-not contrast is a non-color attribute, so the
    // selection survives on monochrome terminals (Tier 10 color-independence).
    let (fill, text_fg, bold) = if filled {
        (welcome_accent(), rgb(20, 24, 32), true)
    } else {
        (rgb(58, 62, 70), rgb(170, 174, 182), false)
    };

    let cap = Style::default().fg(fill);
    let mut body = Style::default().fg(text_fg).bg(fill);
    if bold {
        body = body.add_modifier(Modifier::BOLD);
    }
    // Half-circle caps where they render well; half-block caps (`▐` / `▌`)
    // elsewhere. Half blocks are part of the universally-supported block
    // elements range, so the capsule stays a solid, aligned shape in every
    // terminal font (the corners are square, but nothing floats or clips).
    let (left_cap, right_cap) = if rounded_pill_caps_supported() {
        ("\u{25D6}", "\u{25D7}")
    } else {
        ("\u{2590}", "\u{258C}")
    };
    vec![
        Span::styled(left_cap, cap),
        Span::styled(format!(" {label} "), body),
        Span::styled(right_cap, cap),
    ]
}

/// Build the Yes/No selector as a pair of rounded lozenge pills. The selected
/// option is a bright filled pill; the other is a muted dark capsule. The shape
/// and fill carry the selection visually so no instruction sentence is needed.
fn yes_no_pill_line(yes_highlighted: bool, align: Alignment) -> Line<'static> {
    let mut spans = Vec::new();
    spans.extend(lozenge_pill_spans("Yes", yes_highlighted));
    spans.push(Span::raw("   "));
    spans.extend(lozenge_pill_spans("No", !yes_highlighted));
    Line::from(spans).alignment(align)
}

/// A rounded "Continue" pill button. Rendered above the import list so the user
/// can reach the commit action just by arrowing up out of the rows (no need to
/// read a "Press Enter" instruction). Uses the same lozenge style as the Yes/No
/// pills: a filled accent capsule when `focused`, a hollow outline otherwise.
fn continue_pill_line(focused: bool, align: Alignment) -> Line<'static> {
    Line::from(lozenge_pill_spans("Continue", focused)).alignment(align)
}

/// The summary-screen action row: "Continue" (imports everything, preselected)
/// next to "Import less" (opens the per-login checkbox list) and "Telemetry
/// settings" (opens the telemetry sub-page).
fn import_summary_pills_line(
    focused: crate::tui::ImportSummaryPill,
    align: Alignment,
) -> Line<'static> {
    use crate::tui::ImportSummaryPill as Pill;
    let mut spans = Vec::new();
    spans.extend(lozenge_pill_spans("Continue", focused == Pill::Continue));
    spans.push(Span::raw("   "));
    spans.extend(lozenge_pill_spans(
        "Import less",
        focused == Pill::ImportLess,
    ));
    spans.push(Span::raw("   "));
    spans.extend(lozenge_pill_spans(
        "Telemetry settings",
        focused == Pill::Telemetry,
    ));
    Line::from(spans).alignment(align)
}

/// The telemetry settings sub-page: three stacked pills, most sharing first,
/// each with a dim one-line consequence caption. Reached from the "Telemetry
/// settings" pill on the import summary; Esc returns without changing anything.
fn telemetry_settings_lines(
    highlighted: crate::tui::TelemetryChoice,
    env_forced_off: bool,
    align: Alignment,
) -> Vec<Line<'static>> {
    use crate::tui::TelemetryChoice as Choice;
    let dim = Style::default().fg(dim_color());
    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(
        Line::from(Span::styled(
            "Telemetry settings",
            Style::default()
                .fg(welcome_accent())
                .add_modifier(Modifier::BOLD),
        ))
        .alignment(align),
    );
    lines.push(Line::from(""));

    let options = [
        (
            Choice::Everything,
            "Send everything, including prompts",
            "Helps jcode the most",
        ),
        (
            Choice::NoContent,
            "No prompts or transcripts",
            "Usage stats and crash reports only",
        ),
        (
            Choice::Nothing,
            "Send nothing",
            "We stop seeing crashes and can't fix them",
        ),
    ];
    for (choice, label, caption) in options {
        lines.push(Line::from(lozenge_pill_spans(label, choice == highlighted)).alignment(align));
        lines.push(Line::from(Span::styled(caption, dim)).alignment(align));
        lines.push(Line::from(""));
    }

    if env_forced_off {
        lines.push(
            Line::from(Span::styled(
                "Your environment already disables telemetry (JCODE_NO_TELEMETRY).",
                dim,
            ))
            .alignment(align),
        );
    }
    lines.push(
        Line::from(Span::styled(
            "Esc goes back. Change this later with /telemetry.",
            dim,
        ))
        .alignment(align),
    );
    lines
}

/// Render the read-only detected-login list for the import summary screen: one
/// dim checkmarked row per detected login. No cursor, no columns - the user is
/// just being shown what we found before they hit Continue.
fn import_summary_lines(prompt: &crate::tui::LoginImportPrompt) -> Vec<Line<'static>> {
    let check_style = Style::default()
        .fg(rgb(126, 211, 159))
        .add_modifier(Modifier::BOLD);
    prompt
        .rows
        .iter()
        .map(|row| {
            Line::from(vec![
                Span::styled("✓ ", check_style),
                Span::styled(
                    row.provider_summary.clone(),
                    Style::default().fg(rgb(210, 210, 210)),
                ),
                Span::styled(
                    format!(" ({})", row.source_name),
                    Style::default().fg(dim_color()),
                ),
            ])
            .alignment(Alignment::Center)
        })
        .collect()
}

/// Render the import screen body: a "Yes / No" header row, then one row per
/// detected login. Each login has a circle under the Yes column and a circle
/// under the No column; the *filled* circle is the current choice. Every login
/// defaults to "Yes" (import), so the pre-selected state is obvious: the Yes
/// column is already filled. The cursor row is marked with a `>` gutter, and
/// Left/Right move the choice between Yes and No.
///
/// The header, circles, and gutter are interactive-widget glyphs, not
/// load-bearing prose: the surrounding ASCII copy ("We found N existing
/// logins", "Imports all checked in Ns") already conveys state in plain text.
fn import_two_column_lines(prompt: &crate::tui::LoginImportPrompt) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();

    // Left column width: the widest "<cursor>Provider (source)" entry, so the
    // Yes/No circle columns line up cleanly. The 2-cell cursor gutter is
    // included so the columns do not shift when the cursor moves.
    let left_width = prompt
        .rows
        .iter()
        .map(|r| 2 + r.provider_summary.chars().count() + 2 + r.source_name.chars().count() + 1)
        .max()
        .unwrap_or(0)
        .max(12);

    // Fixed-width Yes/No columns so the header labels sit directly above the
    // circles. Each column is `COL_W` cells with the glyph centered; a small gap
    // separates the two columns.
    const COL_W: usize = 5;
    const GAP: &str = "  ";
    let center_cell = |s: &str| {
        let len = s.chars().count();
        let total = COL_W.saturating_sub(len);
        let left = total / 2;
        let right = total - left;
        format!("{}{}{}", " ".repeat(left), s, " ".repeat(right))
    };

    let yes_color = rgb(126, 211, 159);
    let filled = Style::default().fg(yes_color).add_modifier(Modifier::BOLD);
    let empty = Style::default().fg(dim_color());
    let header_style = Style::default()
        .fg(dim_color())
        .add_modifier(Modifier::BOLD);

    // Header row: blank under the provider column, then "Yes" and "No" labels
    // centered over their circle columns.
    out.push(
        Line::from(vec![
            Span::raw(" ".repeat(left_width)),
            Span::styled(center_cell("Yes"), header_style),
            Span::raw(GAP),
            Span::styled(center_cell("No"), header_style),
        ])
        .alignment(Alignment::Center),
    );

    for (i, row) in prompt.rows.iter().enumerate() {
        // While the Continue pill is focused, no login row shows the `>` gutter.
        let is_cursor = !prompt.continue_focused && i == prompt.cursor;
        // A `> ` gutter marks the row the arrow keys act on.
        let cursor_marker = if is_cursor { "> " } else { "  " };
        let cursor_style = Style::default().fg(welcome_accent());
        let label_style = if row.checked {
            Style::default().fg(rgb(210, 210, 210))
        } else {
            Style::default().fg(dim_color())
        };

        // Compose the left cell, then pad it out to left_width so the circle
        // columns align regardless of provider/source length.
        let left_text = format!(
            "{}{} ({})",
            cursor_marker, row.provider_summary, row.source_name
        );
        let pad = left_width.saturating_sub(left_text.chars().count());

        // Filled circle marks the chosen column; the other is a hollow outline.
        let (yes_glyph, yes_style, no_glyph, no_style) = if row.checked {
            ("●", filled, "○", empty)
        } else {
            ("○", empty, "●", filled)
        };

        let spans: Vec<Span<'static>> = vec![
            Span::styled(cursor_marker, cursor_style),
            Span::styled(row.provider_summary.clone(), label_style),
            Span::styled(
                format!(" ({})", row.source_name),
                Style::default().fg(dim_color()),
            ),
            Span::raw(" ".repeat(pad)),
            Span::styled(center_cell(yes_glyph), yes_style),
            Span::raw(GAP),
            Span::styled(center_cell(no_glyph), no_style),
        ];
        out.push(Line::from(spans).alignment(Alignment::Center));
    }

    out
}

/// Grayed telemetry notice shown at the very top of the onboarding screen.
fn telemetry_header_lines(width: u16) -> Vec<Line<'static>> {
    let align = Alignment::Center;
    let dim = Style::default().fg(dim_color());
    let lines = vec![
        "jcode collects anonymous usage statistics (version, OS, session",
        "activity, and crash reasons). No code, prompts, or personal data.",
        "Change anytime: /telemetry (or export JCODE_NO_TELEMETRY=1)",
    ];
    lines
        .into_iter()
        .map(|text| {
            // Truncate defensively on very narrow terminals.
            let text = if (text.chars().count() as u16) > width.saturating_sub(2) {
                text.chars()
                    .take(width.saturating_sub(3) as usize)
                    .collect::<String>()
                    + "…"
            } else {
                text.to_string()
            };
            Line::from(Span::styled(text, dim)).alignment(align)
        })
        .collect()
}

/// Welcome title line, rendered just above the donut.
fn welcome_title_line() -> Line<'static> {
    Line::from(Span::styled(
        "Welcome to jcode onboarding",
        Style::default()
            .fg(welcome_accent())
            .add_modifier(Modifier::BOLD),
    ))
    .alignment(Alignment::Center)
}

/// Short keyboard hint rendered just below the donut on guided phases. Replaces
/// the old multi-line instruction prose: the interactive pills/rows already show
/// what is selectable, so a one-liner is enough.
fn keyboard_hint_line() -> Line<'static> {
    Line::from(Span::styled(
        "Use your keyboard to navigate.",
        Style::default().fg(dim_color()),
    ))
    .alignment(Alignment::Center)
}

/// The phase-specific body of the welcome screen (everything below the donut and
/// keyboard hint). The title now lives above the donut, so this no longer emits
/// it.
fn welcome_body_lines(app: &dyn TuiState) -> Vec<Line<'static>> {
    let align = Alignment::Center;
    let mut lines: Vec<Line<'static>> = Vec::new();

    use crate::tui::OnboardingWelcomeKind;
    match app.onboarding_welcome_kind() {
        OnboardingWelcomeKind::Login {
            import,
            importing,
            error,
            repair_agent_label,
        } => {
            match import {
                None if importing => {
                    // The user committed the import; it's running. Show progress
                    // instead of the manual-login recovery copy so we never tell
                    // the user to "log in again" right after they chose to import.
                    lines.push(
                        Line::from(Span::styled(
                            "Importing your logins…",
                            Style::default()
                                .fg(welcome_accent())
                                .add_modifier(Modifier::BOLD),
                        ))
                        .alignment(align),
                    );
                    lines.push(
                        Line::from(Span::styled(
                            "Hang tight, this only takes a moment.",
                            Style::default().fg(dim_color()),
                        ))
                        .alignment(align),
                    );
                }
                None if error.is_some() => {
                    // A prior import failed. Explain what happened and give a
                    // concrete, guaranteed next step (Enter opens the picker).
                    let reason = error.unwrap_or_default();
                    lines.push(
                        Line::from(Span::styled(
                            "We couldn't import those logins.",
                            Style::default()
                                .fg(rgb(240, 180, 120))
                                .add_modifier(Modifier::BOLD),
                        ))
                        .alignment(align),
                    );
                    if !reason.is_empty() {
                        lines.push(
                            Line::from(Span::styled(reason, Style::default().fg(dim_color())))
                                .alignment(align),
                        );
                    }
                    lines.push(Line::from(""));
                    lines.push(
                        Line::from(Span::styled(
                            "No problem - you can log in directly.",
                            Style::default()
                                .fg(welcome_accent())
                                .add_modifier(Modifier::BOLD),
                        ))
                        .alignment(align),
                    );
                    lines.push(
                        Line::from(Span::styled(
                            "Press Enter to choose a provider (OpenAI, Anthropic, and more).",
                            Style::default().fg(dim_color()),
                        ))
                        .alignment(align),
                    );
                    // If we detected a coding agent the user recently used, offer
                    // to hand the fix to it (it can run jcode's auth-test and add
                    // the key non-interactively).
                    if let Some(agent) = repair_agent_label {
                        lines.push(
                            Line::from(Span::styled(
                                format!("Press H to have {agent} help fix this for you."),
                                Style::default().fg(welcome_accent()),
                            ))
                            .alignment(align),
                        );
                    }
                }
                None => {
                    lines.push(
                        Line::from(Span::styled(
                            "First, log in to get started.",
                            Style::default()
                                .fg(welcome_accent())
                                .add_modifier(Modifier::BOLD),
                        ))
                        .alignment(align),
                    );
                    lines.push(
                        Line::from(Span::styled(
                            "Press Enter to pick who to log in with (OpenAI, Anthropic, and more).",
                            Style::default().fg(dim_color()),
                        ))
                        .alignment(align),
                    );
                }
                Some(prompt) if !prompt.choosing => {
                    // Telemetry settings sub-page takes over the body while
                    // it is open, then returns to the summary below.
                    if let Some(choice) = prompt.telemetry {
                        lines.extend(telemetry_settings_lines(
                            choice,
                            prompt.telemetry_env_forced_off,
                            align,
                        ));
                        return lines;
                    }
                    // Summary screen (default): show what we detected as a
                    // read-only checkmarked list, then land the user on a
                    // preselected "Continue" pill that imports everything.
                    // "Import less" opens the per-login list; "Telemetry
                    // settings" opens the telemetry sub-page.
                    let found = prompt.rows.len();
                    lines.push(
                        Line::from(Span::styled(
                            format!(
                                "We found {found} existing login{}:",
                                if found == 1 { "" } else { "s" }
                            ),
                            Style::default()
                                .fg(welcome_accent())
                                .add_modifier(Modifier::BOLD),
                        ))
                        .alignment(align),
                    );
                    lines.push(Line::from(""));
                    lines.extend(import_summary_lines(&prompt));
                    lines.push(Line::from(""));
                    lines.push(import_summary_pills_line(prompt.summary_pill, align));
                }
                Some(prompt) => {
                    // Choose mode: a short "Import:" label, the Continue pill,
                    // then the per-login rows. The interactive pill + rows show
                    // what is selectable, so we drop the old instruction prose
                    // and countdown sentence to keep the screen uncluttered.
                    lines.push(
                        Line::from(Span::styled(
                            "Import:",
                            Style::default()
                                .fg(welcome_accent())
                                .add_modifier(Modifier::BOLD),
                        ))
                        .alignment(align),
                    );
                    lines.push(Line::from(""));

                    // Continue pill: arrowing up out of the first row (or down
                    // past the last) focuses it; Enter commits the import.
                    lines.push(continue_pill_line(prompt.continue_focused, align));
                    lines.push(Line::from(""));

                    // Per-login rows: each provider has a Yes/No choice, "Yes"
                    // (import) lit by default. Up/down move, Left/Right flip.
                    lines.extend(import_two_column_lines(&prompt));
                }
            }
            push_esc_skip_hint(&mut lines, align);
            return lines;
        }
        OnboardingWelcomeKind::LoginOpenAi { yes_highlighted } => {
            lines.push(
                Line::from(Span::styled(
                    "Log in to OpenAI?",
                    Style::default()
                        .fg(welcome_accent())
                        .add_modifier(Modifier::BOLD),
                ))
                .alignment(align),
            );
            lines.push(Line::from(""));

            // Rounded Yes/No lozenge pills; the selection is shown visually (the
            // filled capsule), so no instruction sentence is needed.
            lines.push(yes_no_pill_line(yes_highlighted, align));
            // The Esc hint below already says you can log in later with /login,
            // so we don't repeat a "choose No to skip" line here.
            push_esc_skip_hint(&mut lines, align);
            return lines;
        }
        OnboardingWelcomeKind::ContinuePrompt {
            cli_label,
            yes_highlighted,
            seconds_left,
        } => {
            lines.push(Line::from(""));
            lines.push(
                Line::from(Span::styled(
                    format!("Continue where you left off in {cli_label}?"),
                    Style::default()
                        .fg(welcome_accent())
                        .add_modifier(Modifier::BOLD),
                ))
                .alignment(align),
            );
            lines.push(Line::from(""));

            // Rounded Yes/No pills; selection shown visually so the hint stays
            // short. The countdown line below already explains the default.
            lines.push(yes_no_pill_line(yes_highlighted, align));
            lines.push(Line::from(""));
            lines.push(
                Line::from(Span::styled(
                    format!("Opens the resume menu automatically in {seconds_left}s…"),
                    Style::default().fg(dim_color()),
                ))
                .alignment(align),
            );
            push_esc_skip_hint(&mut lines, align);
            return lines;
        }
        OnboardingWelcomeKind::Suggestions => {}
    }

    let suggestions = app.suggestion_prompts();
    if !suggestions.is_empty() {
        lines.push(Line::from(""));
        for (i, (label, prompt)) in suggestions.iter().enumerate() {
            let is_login = prompt.starts_with('/');
            let spans = if is_login {
                vec![
                    Span::styled(
                        format!("{} ", label),
                        Style::default()
                            .fg(welcome_accent())
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("(type {})", prompt),
                        Style::default().fg(dim_color()),
                    ),
                ]
            } else {
                vec![
                    Span::styled(
                        format!("[{}] ", i + 1),
                        Style::default().fg(welcome_accent()),
                    ),
                    Span::styled(label.clone(), Style::default().fg(rgb(200, 200, 200))),
                ]
            };
            lines.push(Line::from(spans).alignment(align));
        }
        if suggestions.len() > 1 {
            lines.push(Line::from(""));
            lines.push(
                Line::from(Span::styled(
                    format!("Press 1-{} or type anything to start", suggestions.len()),
                    Style::default().fg(dim_color()),
                ))
                .alignment(align),
            );
        }
    }

    lines
}

/// Draw the full onboarding welcome screen into `area`.
///
/// Vertical structure (top to bottom):
///   telemetry header, gap, title, donut, keyboard hint, gap, phase body.
/// The title sits directly above the donut and a one-line keyboard hint sits
/// directly below it, so the phase body underneath can stay lean.
pub(super) fn draw_onboarding_welcome(frame: &mut Frame, app: &dyn TuiState, area: Rect) {
    if area.width < 4 || area.height < 6 {
        // Too small for the full treatment: fall back to a minimal welcome.
        let mut lines = vec![welcome_title_line()];
        lines.extend(welcome_body_lines(app));
        frame.render_widget(Paragraph::new(lines), area);
        return;
    }

    let telemetry = telemetry_header_lines(area.width);
    let body = welcome_body_lines(app);
    let telemetry_h = (telemetry.len() as u16).min(TELEMETRY_LINES);
    let body_h = body.len() as u16;
    // Title above the donut, keyboard hint below it. Both are single lines and
    // only shown when there is room for the donut treatment.
    const TITLE_H: u16 = 1;
    const HINT_H: u16 = 1;

    // Donut shrinks if the area is short so the welcome text always fits. The
    // title + hint lines that hug the donut are part of the reserved chrome.
    let donut_h = DONUT_HEIGHT.min(
        area.height
            .saturating_sub(telemetry_h + TITLE_H + HINT_H + body_h + GAP * 2 + 1),
    );
    let show_donut_block = donut_h > 0;

    let used = if show_donut_block {
        telemetry_h + GAP + TITLE_H + donut_h + HINT_H + GAP + body_h
    } else {
        telemetry_h + GAP + body_h
    };
    let pad_top = area.height.saturating_sub(used) / 2;

    let mut constraints = vec![Constraint::Length(pad_top), Constraint::Length(telemetry_h)];
    if show_donut_block {
        constraints.push(Constraint::Length(GAP));
        constraints.push(Constraint::Length(TITLE_H));
        constraints.push(Constraint::Length(donut_h));
        constraints.push(Constraint::Length(HINT_H));
    }
    constraints.push(Constraint::Length(GAP));
    constraints.push(Constraint::Length(body_h));
    constraints.push(Constraint::Min(0));

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    // chunks[0] = top pad, [1] = telemetry, then optional gap/title/donut/hint,
    // gap, body.
    frame.render_widget(
        Paragraph::new(telemetry).alignment(Alignment::Center),
        chunks[1],
    );

    let mut idx = 2;
    if show_donut_block {
        idx += 1; // skip gap chunk
        frame.render_widget(
            Paragraph::new(welcome_title_line()).alignment(Alignment::Center),
            chunks[idx],
        );
        idx += 1; // title -> donut
        animations::draw_idle_animation(frame, app, chunks[idx]);
        idx += 1; // donut -> hint
        frame.render_widget(
            Paragraph::new(keyboard_hint_line()).alignment(Alignment::Center),
            chunks[idx],
        );
        idx += 1; // hint -> gap
    }
    idx += 1; // skip gap chunk
    frame.render_widget(
        Paragraph::new(body).alignment(Alignment::Center),
        chunks[idx],
    );
}
