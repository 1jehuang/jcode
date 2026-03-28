use crate::tui::color_support::rgb;
use crate::tui::workspace_map::{VisibleWorkspaceRow, WorkspaceSessionVisualState};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
};

const TILE_WIDTH: u16 = 1;
const TILE_HEIGHT: u16 = 1;
const COL_GAP: u16 = 1;
const ROW_GAP: u16 = 1;

pub fn preferred_size(rows: &[VisibleWorkspaceRow]) -> (u16, u16) {
    let max_tiles = rows.iter().map(|row| row.sessions.len()).max().unwrap_or(0) as u16;
    let width = if max_tiles == 0 {
        TILE_WIDTH
    } else {
        max_tiles * TILE_WIDTH + max_tiles.saturating_sub(1) * COL_GAP
    };
    let height = rows.len() as u16 * TILE_HEIGHT + rows.len().saturating_sub(1) as u16 * ROW_GAP;
    (width, height)
}

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
            let area_right = area.x.saturating_add(area.width);
            let area_bottom = area.y.saturating_add(area.height);
            if x >= area_right || y >= area_bottom {
                continue;
            }
            let width = area_right.saturating_sub(x).min(TILE_WIDTH);
            let height = area_bottom.saturating_sub(y).min(TILE_HEIGHT);
            if width == 0 || height == 0 {
                continue;
            }
            placements.push(WorkspaceTilePlacement {
                workspace: row.workspace,
                session_index,
                rect: Rect::new(x, y, width, height),
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
    if placement.rect.width == 0 || placement.rect.height == 0 {
        return;
    }

    let fg = tile_color(
        placement.state,
        placement.focused,
        placement.current_workspace,
        tick,
    );
    let symbol = tile_symbol(placement.state, placement.focused, tick);
    let style = if placement.focused {
        Style::default().fg(fg).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(fg)
    };

    for y in placement.rect.y..placement.rect.y.saturating_add(placement.rect.height) {
        for x in placement.rect.x..placement.rect.x.saturating_add(placement.rect.width) {
            buf[(x, y)].set_symbol(symbol).set_style(style);
        }
    }
}

fn tile_symbol(state: WorkspaceSessionVisualState, focused: bool, tick: u64) -> &'static str {
    match state {
        WorkspaceSessionVisualState::Running => match tick % 4 {
            0 => "◴",
            1 => "◷",
            2 => "◶",
            _ => "◵",
        },
        _ if focused => "■",
        _ => "▪",
    }
}

fn tile_color(
    state: WorkspaceSessionVisualState,
    focused: bool,
    current_workspace: bool,
    tick: u64,
) -> Color {
    match state {
        WorkspaceSessionVisualState::Running => {
            if focused {
                if tick % 2 == 0 {
                    rgb(180, 220, 255)
                } else {
                    rgb(130, 170, 220)
                }
            } else if tick % 2 == 0 {
                rgb(140, 200, 255)
            } else {
                rgb(90, 140, 190)
            }
        }
        WorkspaceSessionVisualState::Error => {
            if focused {
                rgb(255, 160, 160)
            } else {
                rgb(255, 120, 120)
            }
        }
        WorkspaceSessionVisualState::Waiting => {
            if focused {
                rgb(255, 225, 150)
            } else {
                rgb(255, 210, 120)
            }
        }
        WorkspaceSessionVisualState::Completed => {
            if focused {
                rgb(160, 240, 180)
            } else {
                rgb(120, 220, 140)
            }
        }
        WorkspaceSessionVisualState::Detached => {
            if focused {
                rgb(200, 200, 215)
            } else {
                rgb(170, 170, 190)
            }
        }
        WorkspaceSessionVisualState::Idle => {
            if focused {
                rgb(220, 220, 240)
            } else if current_workspace {
                rgb(150, 150, 165)
            } else {
                rgb(95, 95, 110)
            }
        }
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
    fn render_workspace_map_uses_square_for_focused_tile() {
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
        assert!(symbols.contains("■"));
    }

    #[test]
    fn render_workspace_map_colors_completed_tiles_green() {
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

        let has_greenish_fg = buf.content().iter().any(|cell| {
            matches!(cell.style().fg, Some(ratatui::style::Color::Rgb(r, g, b)) if g > r && g > b)
        });
        assert!(has_greenish_fg);
    }

    #[test]
    fn running_tile_uses_spinner_frames() {
        let rows = vec![row(
            0,
            true,
            Some(0),
            vec![WorkspaceSessionTile::with_state(
                "fox",
                WorkspaceSessionVisualState::Running,
            )],
        )];
        let mut buf_a = Buffer::empty(Rect::new(0, 0, 20, 6));
        render_workspace_map(&mut buf_a, Rect::new(0, 0, 20, 6), &rows, 0);
        let mut buf_b = Buffer::empty(Rect::new(0, 0, 20, 6));
        render_workspace_map(&mut buf_b, Rect::new(0, 0, 20, 6), &rows, 1);

        let symbols_a: String = buf_a
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<Vec<_>>()
            .join("");
        let symbols_b: String = buf_b
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<Vec<_>>()
            .join("");
        assert_ne!(symbols_a, symbols_b);
    }

    #[test]
    fn placements_clip_when_area_is_narrower_than_full_row() {
        let rows = vec![row(
            0,
            true,
            Some(0),
            vec![
                WorkspaceSessionTile::new("fox"),
                WorkspaceSessionTile::new("bear"),
                WorkspaceSessionTile::new("owl"),
            ],
        )];
        let area = Rect::new(0, 0, 12, 6);
        let placements = compute_workspace_tile_placements(area, &rows);
        assert!(!placements.is_empty());
        let right = area.x + area.width;
        assert!(placements.iter().all(|placement| placement.rect.x < right));
        assert!(
            placements
                .iter()
                .all(|placement| placement.rect.x + placement.rect.width <= right)
        );
    }
}
