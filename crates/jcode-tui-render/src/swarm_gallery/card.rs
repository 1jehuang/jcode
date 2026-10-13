//! Swarm chat cards and live cards for transcript and detail views.

use ratatui::prelude::*;

use jcode_tui_style::color::rgb;

use super::hover::hovered_detail_body;
use super::types::GalleryMember;
use super::util::{
    card_status_glyph, card_status_label, clamp_line_to_width, disp_w, format_elapsed,
    format_model, format_route, role_color, sort_members_for_display, status_accent, status_glyph,
};

pub fn render_swarm_chat_cards(members: &[GalleryMember], width: usize) -> Vec<Line<'static>> {
    if members.is_empty() || width < 8 {
        return Vec::new();
    }

    let mut out = Vec::new();
    for member in sort_members_for_display(members) {
        let accent =
            role_color(member.role.as_deref()).unwrap_or_else(|| status_accent(&member.status));
        let lead = format!(
            "    {} {} ",
            member.icon.as_deref().unwrap_or("🐝"),
            card_status_glyph(&member.status)
        );
        let label = member.label.clone();

        // Stable runtime metadata (model and provider/auth route) is fixed at
        // spawn time, so it can live on the transcript card without making old
        // chat rows churn. Drop trailing pieces first when width is tight.
        let mut metadata = vec![card_status_label(&member.status).to_string()];
        if let Some(model) = member
            .model
            .as_deref()
            .filter(|model| !model.trim().is_empty())
        {
            metadata.push(format_model(model));
        }
        if let Some(route) = format_route(member.provider.as_deref(), member.auth_method.as_deref())
        {
            metadata.push(route);
        }
        let mut tail = format!(" · {}", metadata.join(" · "));
        while metadata.len() > 1 && disp_w(&lead) + disp_w(&label) + disp_w(&tail) > width {
            metadata.pop();
            tail = format!(" · {}", metadata.join(" · "));
        }

        let mut header = vec![
            Span::styled(lead.clone(), Style::default().fg(rgb(255, 200, 100))),
            Span::styled(
                label.clone(),
                Style::default().fg(accent).add_modifier(Modifier::BOLD),
            ),
        ];
        let consumed = disp_w(&lead) + disp_w(&label);
        if consumed + disp_w(&tail) <= width {
            header.push(Span::styled(tail, Style::default().fg(rgb(150, 150, 160))));
        }
        out.push(Line::from(header));
    }

    for line in &mut out {
        clamp_line_to_width(line, width);
    }
    out
}

/// Render the selected member's detailed live card for the dedicated swarm
/// page. Active states use the animated spinner and the body includes current
/// todos/tool activity, runtime metadata, and the streamed output tail.
pub fn render_swarm_live_card(
    member: &GalleryMember,
    spinner_frame: usize,
    width: usize,
    max_height: usize,
) -> Vec<Line<'static>> {
    if width < 8 || max_height == 0 {
        return Vec::new();
    }

    let accent =
        role_color(member.role.as_deref()).unwrap_or_else(|| status_accent(&member.status));
    let mut metadata = Vec::new();
    if let Some(elapsed) = member.elapsed_secs {
        metadata.push(format_elapsed(elapsed));
    }
    if let Some(model) = member
        .model
        .as_deref()
        .filter(|model| !model.trim().is_empty())
    {
        metadata.push(format_model(model));
    }
    if let Some(route) = format_route(member.provider.as_deref(), member.auth_method.as_deref()) {
        metadata.push(route);
    }
    if let Some(effort) = member
        .effort
        .as_deref()
        .filter(|effort| !effort.trim().is_empty())
    {
        metadata.push(effort.to_string());
    }

    let lead = format!(
        "  {} {} ",
        member.icon.as_deref().unwrap_or("🐝"),
        status_glyph(&member.status, spinner_frame)
    );
    let label = member.label.clone();
    let mut tail = if metadata.is_empty() {
        String::new()
    } else {
        format!(" · {}", metadata.join(" · "))
    };
    while metadata.len() > 1 && disp_w(&lead) + disp_w(&label) + disp_w(&tail) > width {
        metadata.pop();
        tail = format!(" · {}", metadata.join(" · "));
    }

    let mut header = vec![
        Span::styled(lead.clone(), Style::default().fg(rgb(255, 200, 100))),
        Span::styled(
            label.clone(),
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        ),
    ];
    let consumed = disp_w(&lead) + disp_w(&label);
    if consumed + disp_w(&tail) <= width {
        header.push(Span::styled(tail, Style::default().fg(rgb(150, 150, 160))));
    }

    let mut out = vec![Line::from(header)];
    let body_budget = max_height.saturating_sub(1);
    if body_budget > 0 {
        out.extend(hovered_detail_body(member, None, width, body_budget, false));
    }
    out.truncate(max_height);
    for line in &mut out {
        clamp_line_to_width(line, width);
    }
    out
}
