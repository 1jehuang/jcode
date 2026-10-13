//! Gallery grid rendering: header, tile conversion, and full gallery layout.

use ratatui::prelude::*;

use jcode_tui_style::color::rgb;

use crate::swarm_tiles::{SwarmGalleryConfig, SwarmTile, render_swarm_gallery};

use super::types::GalleryMember;
use super::util::{
    clamp_line_to_width, is_active_status, role_color, role_glyph, sort_members_for_display,
    status_accent,
};

pub fn gallery_header(total: usize, active: usize) -> Line<'static> {
    Line::from(vec![
        Span::styled("🐝 ", Style::default().fg(rgb(255, 200, 100))),
        Span::styled(
            format!(
                "{} agent{}{}",
                total,
                if total == 1 { "" } else { "s" },
                if active > 0 {
                    format!(" · {active} active")
                } else {
                    String::new()
                }
            ),
            Style::default().fg(rgb(160, 160, 170)),
        ),
    ])
}

pub fn members_to_tiles(members: &[GalleryMember]) -> Vec<SwarmTile> {
    sort_members_for_display(members)
        .into_iter()
        .map(|m| {
            let accent = role_color(m.role.as_deref()).unwrap_or_else(|| status_accent(&m.status));
            let mut tile =
                SwarmTile::new(m.label.clone(), m.status.clone(), accent).with_body(m.body.clone());
            if let Some(glyph) = role_glyph(m.role.as_deref()) {
                tile = tile.with_role_glyph(glyph);
            }
            if let Some(rc) = role_color(m.role.as_deref()) {
                tile = tile.with_role_color(rc);
            }
            tile
        })
        .collect()
}

pub fn render_gallery(
    members: &[GalleryMember],
    width: usize,
    max_height: usize,
    selected: Option<usize>,
) -> Vec<Line<'static>> {
    if members.is_empty() {
        return Vec::new();
    }
    let tiles = members_to_tiles(members);
    let active = members
        .iter()
        .filter(|m| is_active_status(&m.status))
        .count();
    let header = gallery_header(members.len(), active);
    let cfg = SwarmGalleryConfig {
        max_height: max_height.saturating_sub(1).max(4),
        selected,
        ..Default::default()
    };
    let mut out = render_swarm_gallery(&tiles, width, &cfg, Some(header));
    // The grid cells are width-bounded already, but the header (and any
    // degenerate-width artifacts) are not. Enforce the bound uniformly.
    for line in &mut out {
        clamp_line_to_width(line, width);
    }
    out
}
