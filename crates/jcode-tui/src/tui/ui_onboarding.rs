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
use super::{dim_color, header_name_color};
use crate::tui::TuiState;
use crate::tui::color_support::rgb;
use ratatui::{prelude::*, widgets::Paragraph};

const DONUT_HEIGHT: u16 = 12;
const TELEMETRY_LINES: u16 = 4;
const GAP: u16 = 1;

/// Accent color for the welcome title.
fn welcome_accent() -> Color {
    rgb(138, 180, 248)
}

/// Grayed telemetry notice shown at the very top of the onboarding screen.
fn telemetry_header_lines(width: u16) -> Vec<Line<'static>> {
    let align = Alignment::Center;
    let dim = Style::default().fg(dim_color());
    let lines = vec![
        "jcode collects anonymous usage statistics (version, OS, session",
        "activity, and crash reasons). No code, prompts, or personal data.",
        "Opt out anytime: export JCODE_NO_TELEMETRY=1",
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

/// Welcome title + the getting-started prompt/suggestions.
fn welcome_body_lines(app: &dyn TuiState) -> Vec<Line<'static>> {
    let align = Alignment::Center;
    let mut lines: Vec<Line<'static>> = Vec::new();

    lines.push(
        Line::from(Span::styled(
            "Welcome to jcode onboarding",
            Style::default()
                .fg(welcome_accent())
                .add_modifier(Modifier::BOLD),
        ))
        .alignment(align),
    );
    lines.push(
        Line::from(Span::styled(
            "Let's get you set up.",
            Style::default().fg(header_name_color()),
        ))
        .alignment(align),
    );

    use crate::tui::OnboardingWelcomeKind;
    match app.onboarding_welcome_kind() {
        OnboardingWelcomeKind::Login { import } => {
            lines.push(Line::from(""));
            match import {
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
                            "Press Enter to choose a provider.",
                            Style::default().fg(dim_color()),
                        ))
                        .alignment(align),
                    );
                }
                Some(prompt) => {
                    lines.push(
                        Line::from(Span::styled(
                            format!(
                                "We found {} existing login{}.",
                                prompt.total,
                                if prompt.total == 1 { "" } else { "s" },
                            ),
                            Style::default()
                                .fg(welcome_accent())
                                .add_modifier(Modifier::BOLD),
                        ))
                        .alignment(align),
                    );
                    lines.push(Line::from(""));
                    lines.push(
                        Line::from(Span::styled(
                            format!("Login {} of {}", prompt.position, prompt.total),
                            Style::default().fg(dim_color()),
                        ))
                        .alignment(align),
                    );
                    lines.push(
                        Line::from(vec![
                            Span::styled("Import ", Style::default().fg(rgb(200, 200, 200))),
                            Span::styled(
                                prompt.provider_summary.clone(),
                                Style::default()
                                    .fg(welcome_accent())
                                    .add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(
                                format!(" ({})?", prompt.source_name),
                                Style::default().fg(dim_color()),
                            ),
                        ])
                        .alignment(align),
                    );
                    lines.push(Line::from(""));

                    // Yes / No options; the highlighted one is bold + accented.
                    let (yes_style, no_style) = if prompt.yes_highlighted {
                        (
                            Style::default()
                                .fg(welcome_accent())
                                .add_modifier(Modifier::BOLD | Modifier::REVERSED),
                            Style::default().fg(dim_color()),
                        )
                    } else {
                        (
                            Style::default().fg(dim_color()),
                            Style::default()
                                .fg(welcome_accent())
                                .add_modifier(Modifier::BOLD | Modifier::REVERSED),
                        )
                    };
                    lines.push(
                        Line::from(vec![
                            Span::styled("  Yes  ", yes_style),
                            Span::raw("   "),
                            Span::styled("  No  ", no_style),
                        ])
                        .alignment(align),
                    );
                    lines.push(Line::from(""));
                    lines.push(
                        Line::from(Span::styled(
                            "Left/right or h/l to move, Enter or Space to choose (y / n also work).",
                            Style::default().fg(dim_color()),
                        ))
                        .alignment(align),
                    );
                    lines.push(
                        Line::from(Span::styled(
                            format!("Auto-selects in {}s.", prompt.seconds_left),
                            Style::default().fg(dim_color()),
                        ))
                        .alignment(align),
                    );
                }
            }
            return lines;
        }
        OnboardingWelcomeKind::TelemetryConsent {
            yes_highlighted,
            seconds_left,
        } => {
            lines.push(Line::from(""));
            lines.push(
                Line::from(Span::styled(
                    "Help improve jcode?",
                    Style::default()
                        .fg(welcome_accent())
                        .add_modifier(Modifier::BOLD),
                ))
                .alignment(align),
            );
            lines.push(Line::from(""));
            lines.push(
                Line::from(Span::styled(
                    "Share your prompts and transcripts so we can improve the product.",
                    Style::default().fg(rgb(200, 200, 200)),
                ))
                .alignment(align),
            );
            lines.push(
                Line::from(Span::styled(
                    "This is optional and off by default. You can change it later.",
                    Style::default().fg(dim_color()),
                ))
                .alignment(align),
            );
            lines.push(Line::from(""));

            // Yes / No options; the highlighted one is bold + accented.
            let (yes_style, no_style) = if yes_highlighted {
                (
                    Style::default()
                        .fg(welcome_accent())
                        .add_modifier(Modifier::BOLD | Modifier::REVERSED),
                    Style::default().fg(dim_color()),
                )
            } else {
                (
                    Style::default().fg(dim_color()),
                    Style::default()
                        .fg(welcome_accent())
                        .add_modifier(Modifier::BOLD | Modifier::REVERSED),
                )
            };
            lines.push(
                Line::from(vec![
                    Span::styled("  Yes  ", yes_style),
                    Span::raw("   "),
                    Span::styled("  No  ", no_style),
                ])
                .alignment(align),
            );
            lines.push(Line::from(""));
            lines.push(
                Line::from(Span::styled(
                    "Left/right or h/l to move, Enter or Space to choose (y / n also work).",
                    Style::default().fg(dim_color()),
                ))
                .alignment(align),
            );
            lines.push(
                Line::from(Span::styled(
                    format!("Declines automatically in {seconds_left}s."),
                    Style::default().fg(dim_color()),
                ))
                .alignment(align),
            );
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

            // Yes / No options; the highlighted one is bold + accented.
            let (yes_style, no_style) = if yes_highlighted {
                (
                    Style::default()
                        .fg(welcome_accent())
                        .add_modifier(Modifier::BOLD | Modifier::REVERSED),
                    Style::default().fg(dim_color()),
                )
            } else {
                (
                    Style::default().fg(dim_color()),
                    Style::default()
                        .fg(welcome_accent())
                        .add_modifier(Modifier::BOLD | Modifier::REVERSED),
                )
            };
            lines.push(
                Line::from(vec![
                    Span::styled("  Yes  ", yes_style),
                    Span::raw("   "),
                    Span::styled("  No  ", no_style),
                ])
                .alignment(align),
            );
            lines.push(Line::from(""));
            lines.push(
                Line::from(Span::styled(
                    "Left/right or h/l to move, Enter or Space to choose (y / n also work).",
                    Style::default().fg(dim_color()),
                ))
                .alignment(align),
            );
            lines.push(
                Line::from(Span::styled(
                    format!("Opens the resume menu automatically in {seconds_left}s…"),
                    Style::default().fg(dim_color()),
                ))
                .alignment(align),
            );
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
pub(super) fn draw_onboarding_welcome(frame: &mut Frame, app: &dyn TuiState, area: Rect) {
    if area.width < 4 || area.height < 6 {
        // Too small for the full treatment: fall back to a minimal welcome.
        let lines = welcome_body_lines(app);
        frame.render_widget(Paragraph::new(lines), area);
        return;
    }

    let telemetry = telemetry_header_lines(area.width);
    let body = welcome_body_lines(app);
    let telemetry_h = (telemetry.len() as u16).min(TELEMETRY_LINES);
    let body_h = body.len() as u16;

    // Donut shrinks if the area is short so the welcome text always fits.
    let donut_h = DONUT_HEIGHT.min(
        area.height
            .saturating_sub(telemetry_h + body_h + GAP * 2 + 1),
    );
    let donut_h = donut_h.max(0);

    let used = telemetry_h + GAP + donut_h + GAP + body_h;
    let pad_top = area.height.saturating_sub(used) / 2;

    let mut constraints = vec![Constraint::Length(pad_top), Constraint::Length(telemetry_h)];
    if donut_h > 0 {
        constraints.push(Constraint::Length(GAP));
        constraints.push(Constraint::Length(donut_h));
    }
    constraints.push(Constraint::Length(GAP));
    constraints.push(Constraint::Length(body_h));
    constraints.push(Constraint::Min(0));

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    // chunks[0] = top pad, [1] = telemetry, then optional gap+donut, gap, body.
    frame.render_widget(
        Paragraph::new(telemetry).alignment(Alignment::Center),
        chunks[1],
    );

    let mut idx = 2;
    if donut_h > 0 {
        // skip gap chunk
        idx += 1;
        animations::draw_idle_animation(frame, app, chunks[idx]);
        idx += 1;
    }
    // skip gap chunk
    idx += 1;
    frame.render_widget(
        Paragraph::new(body).alignment(Alignment::Center),
        chunks[idx],
    );
}
