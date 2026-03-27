use crate::tui::color_support::rgb;
use crate::tui::workspace_map::{VisibleWorkspaceRow, WorkspaceSessionVisualState};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
};

const TILE_WIDTH: u16 = 8;
const TILE_HEIGHT: u16 = 2;
const COL_GAP: u16 = 2;
const ROW_GAP: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkspaceTilePlacement {
    pub workspace: i32,
    pub session_index: usize,
    pub rect: Rect,
    pub focused: bool,
    pub current_workspace: bool,
    pub state: WorkspaceSessionVisualState,
}

pub fn compute_workspace_tile_placements(
    area: Rect,
    rows: &[VisibleWorkspaceRow],
) -> Vec<WorkspaceTilePlacement> {
    if area.width == 0 || area.height == 0 || rows.is_empty() {
        return Vec::new();
    }

    let row_stride = TILE_HEIGHT + ROW_GAP;
    let total_height = rows
        .len()
        .saturating_mul(TILE_HEIGHT as usize)
        .saturating_add(rows.len().saturating_sub(1) * ROW_GAP as usize)
        .min(u16::MAX as usize) as u16;
    let top_offset = area.y + area.height.saturating_sub(total_height) / 2;

    let mut placements = Vec::new();
    for (row_idx, row) in rows.iter().enumerate() {
        let tile_count = row.sessions.len() as u16;
        let row_width = if tile_count == 0 {
            0
        } else {
            tile_count * TILE_WIDTH + tile_count.saturating_sub(1) * COL_GAP
        };
        let left_offset = area.x + area.width.saturating_sub(row_width) / 2;
        let y = top_offset + (row_idx as u16 * row_stride);

        for (session_index, session) in row.sessions.iter().enumerate() {
            let x = left_offset + (session_index as u16 * (TILE_WIDTH + COL_GAP));
            placements.push(WorkspaceTilePlacement {
                workspace: row.workspace,
                session_index,
                rect: Rect::new(x, y, TILE_WIDTH.min(area.width), TILE_HEIGHT.min(area.height)),
                focused: row.focused_index == Some(session_index),
                current_workspace: row.is_current,
                state: session.state,
            });
        }
    }

    placements
}

pub fn render_workspace_map(buf: &mut Buffer, area: Rect, rows: &[VisibleWorkspaceRow], tick: u64) {
    clear_area(buf, area);
    for placement in compute_workspace_tile_placements(area, rows) {
        draw_workspace_tile(buf, placement, tick);
    }
}

fn clear_area(buf: &mut Buffer, area: Rect) {
    for y in area.y..area.y.saturating_add(area.height) {
        for x in area.x..area.x.saturating_add(area.width) {
            buf[(x, y)].set_symbol(" ").set_style(Style::default());
        }
    }
}

fn draw_workspace_tile(buf: &mut Buffer, placement: WorkspaceTilePlacement, tick: u64) {
    if placement.rect.width < 2 || placement.rect.height < 2 {
        return;
    }

    let border_color = border_color(placement.state, placement.focused, placement.current_workspace, tick);
    let fill_color = fill_color(placement.state);
    let glyphs = if placement.focused {
        ['╔', '╗', '╚', '╝', '═', '║']
    } else {
        ['┌', '┐', '└', '┘', '─', '│']
    };

    let x0 = placement.rect.x;
    let y0 = placement.rect.y;
    let x1 = placement.rect.x + placement.rect.width - 1;
    let y1 = placement.rect.y + placement.rect.height - 1;

    // fill interior/background
    for y in y0..=y1 {
        for x in x0..=x1 {
            let mut style = Style::default();
            if let Some(bg) = fill_color {
                style = style.bg(bg);
            }
            buf[(x, y)].set_symbol(" ").set_style(style);
        }
    }

    buf[(x0, y0)]
        .set_symbol(&glyphs[0].to_string())
        .set_style(Style::default().fg(border_color).bg(fill_color.unwrap_or(Color::Reset)));
    buf[(x1, y0)]
        .set_symbol(&glyphs[1].to_string())
        .set_style(Style::default().fg(border_color).bg(fill_color.unwrap_or(Color::Reset)));
    buf[(x0, y1)]
        .set_symbol(&glyphs[2].to_string())
        .set_style(Style::default().fg(border_color).bg(fill_color.unwrap_or(Color::Reset)));
    buf[(x1, y1)]
        .set_symbol(&glyphs[3].to_string())
        .set_style(Style::default().fg(border_color).bg(fill_color.unwrap_or(Color::Reset)));

    for x in (x0 + 1)..x1 {
        buf[(x, y0)]
            .set_symbol(&glyphs[4].to_string())
            .set_style(Style::default().fg(border_color).bg(fill_color.unwrap_or(Color::Reset)));
        buf[(x, y1)]
            .set_symbol(&glyphs[4].to_string())
            .set_style(Style::default().fg(border_color).bg(fill_color.unwrap_or(Color::Reset)));
    }
    for y in (y0 + 1)..y1 {
        buf[(x0, y)]
            .set_symbol(&glyphs[5].to_string())
            .set_style(Style::default().fg(border_color).bg(fill_color.unwrap_or(Color::Reset)));
        buf[(x1, y)]
            .set_symbol(&glyphs[5].to_string())
            .set_style(Style::default().fg(border_color).bg(fill_color.unwrap_or(Color::Reset)));
    }
}

fn border_color(
    state: WorkspaceSessionVisualState,
    focused: bool,
    current_workspace: bool,
    tick: u64,
) -> Color {
    if focused {
        return rgb(220, 220, 240);
    }
    match state {
        WorkspaceSessionVisualState::Running => {
            if tick % 2 == 0 {
                rgb(140, 200, 255)
            } else {
                rgb(90, 140, 190)
            }
        }
        WorkspaceSessionVisualState::Error => rgb(255, 120, 120),
        WorkspaceSessionVisualState::Waiting => rgb(255, 210, 120),
        WorkspaceSessionVisualState::Completed => rgb(120, 220, 140),
        WorkspaceSessionVisualState::Detached => rgb(170, 170, 190),
        WorkspaceSessionVisualState::Idle => {
            if current_workspace {
                rgb(150, 150, 165)
            } else {
                rgb(95, 95, 110)
            }
        }
    }
}

fn fill_color(state: WorkspaceSessionVisualState) -> Option<Color> {
    match state {
        WorkspaceSessionVisualState::Completed => Some(rgb(40, 90, 50)),
        WorkspaceSessionVisualState::Waiting => Some(rgb(90, 75, 30)),
        WorkspaceSessionVisualState::Error => Some(rgb(95, 35, 35)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{compute_workspace_tile_placements, render_workspace_map};
    use crate::tui::workspace_map::{
        VisibleWorkspaceRow, WorkspaceSessionTile, WorkspaceSessionVisualState,
    };
    use ratatui::{buffer::Buffer, layout::Rect};

    fn row(
        workspace: i32,
        is_current: bool,
        focused_index: Option<usize>,
        sessions: Vec<WorkspaceSessionTile>,
    ) -> VisibleWorkspaceRow {
        VisibleWorkspaceRow {
            workspace,
            is_current,
            focused_index,
            sessions,
        }
    }

    #[test]
    fn placements_center_rows_and_preserve_order() {
        let rows = vec![row(
            0,
            true,
            Some(1),
            vec![
                WorkspaceSessionTile::new("fox"),
                WorkspaceSessionTile::new("bear"),
                WorkspaceSessionTile::new("owl"),
            ],
        )];
        let placements = compute_workspace_tile_placements(Rect::new(0, 0, 40, 8), &rows);
        assert_eq!(placements.len(), 3);
        assert!(placements[0].rect.x < placements[1].rect.x);
        assert!(placements[1].rect.x < placements[2].rect.x);
        assert!(placements[1].focused);
    }

    #[test]
    fn render_workspace_map_uses_double_border_for_focused_tile() {
        let rows = vec![row(
            0,
            true,
            Some(0),
            vec![WorkspaceSessionTile::new("fox")],
        )];
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 6));
        render_workspace_map(&mut buf, Rect::new(0, 0, 20, 6), &rows, 0);

        let symbols: String = buf
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<Vec<_>>()
            .join("");
        assert!(symbols.contains("╔"));
        assert!(symbols.contains("╝"));
    }

    #[test]
    fn render_workspace_map_fills_completed_tiles() {
        let rows = vec![row(
            0,
            true,
            Some(0),
            vec![WorkspaceSessionTile::with_state(
                "fox",
                WorkspaceSessionVisualState::Completed,
            )],
        )];
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 6));
        render_workspace_map(&mut buf, Rect::new(0, 0, 20, 6), &rows, 0);

        let has_greenish_bg = buf.content().iter().any(|cell| {
            matches!(cell.style().bg, Some(ratatui::style::Color::Rgb(r, g, b)) if g > r && g > b)
        });
        assert!(has_greenish_bg);
    }
}
