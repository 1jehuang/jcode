//! Tile rendered swarm-replay panes into one frame sequence for video export.
//!
//! Lives in the presentation crate because it composes `ratatui` buffers.
//! The replay timeline model itself stays in `jcode-app-core::replay`.

use ratatui::{buffer::Buffer, layout::Rect};

#[derive(Debug, Clone)]
pub struct SwarmPaneFrames {
    pub session_id: String,
    pub title: String,
    pub frames: Vec<(f64, Buffer)>,
}

pub fn compose_swarm_buffers(
    pane_frames: &[SwarmPaneFrames],
    width: u16,
    height: u16,
    fps: u32,
    cols: u16,
) -> Vec<(f64, Buffer)> {
    if pane_frames.is_empty() {
        return Vec::new();
    }

    let fps = fps.max(1);
    let frame_step = 1.0 / fps as f64;
    let end_time = pane_frames
        .iter()
        .filter_map(|pane| pane.frames.last().map(|(t, _)| *t))
        .fold(0.0, f64::max);

    let pane_count = pane_frames.len() as u16;
    let cols = cols.clamp(1, pane_count.max(1));
    let rows = pane_count.div_ceil(cols).max(1);
    let pane_width = (width / cols).max(1);
    let pane_height = (height / rows).max(1);

    let mut output = Vec::new();
    let mut t = 0.0;
    while t <= end_time + frame_step {
        let mut canvas = Buffer::empty(Rect::new(0, 0, width, height));
        for (idx, pane) in pane_frames.iter().enumerate() {
            let idx = idx as u16;
            let col = idx % cols;
            let row = idx / cols;
            let x = col * pane_width;
            let y = row * pane_height;
            let area = Rect::new(
                x,
                y,
                if col == cols - 1 {
                    width - x
                } else {
                    pane_width
                },
                if row == rows - 1 {
                    height - y
                } else {
                    pane_height
                },
            );
            if let Some(buf) = buffer_at_time(&pane.frames, t) {
                blit_buffer(&mut canvas, area, buf);
            }
        }
        output.push((t, canvas));
        t += frame_step;
    }

    output
}

fn buffer_at_time(frames: &[(f64, Buffer)], t: f64) -> Option<&Buffer> {
    let mut current = None;
    for (frame_t, buf) in frames {
        if *frame_t <= t {
            current = Some(buf);
        } else {
            break;
        }
    }
    current.or_else(|| frames.first().map(|(_, buf)| buf))
}

fn blit_buffer(dst: &mut Buffer, area: Rect, src: &Buffer) {
    for sy in 0..area.height.min(src.area.height) {
        for sx in 0..area.width.min(src.area.width) {
            let dx = area.x + sx;
            let dy = area.y + sy;
            if let (Some(src_cell), Some(dst_cell)) = (src.cell((sx, sy)), dst.cell_mut((dx, dy))) {
                *dst_cell = src_cell.clone();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Style;

    #[test]
    fn compose_swarm_buffers_combines_panes() {
        let mut left = Buffer::empty(Rect::new(0, 0, 4, 2));
        left[(0, 0)].set_symbol("L").set_style(Style::default());
        let mut right = Buffer::empty(Rect::new(0, 0, 4, 2));
        right[(0, 0)].set_symbol("R").set_style(Style::default());

        let panes = vec![
            SwarmPaneFrames {
                session_id: "left".to_string(),
                title: "left".to_string(),
                frames: vec![(0.0, left)],
            },
            SwarmPaneFrames {
                session_id: "right".to_string(),
                title: "right".to_string(),
                frames: vec![(0.0, right)],
            },
        ];

        let frames = compose_swarm_buffers(&panes, 8, 2, 1, 2);
        assert!(!frames.is_empty());
        let buf = &frames[0].1;
        assert_eq!(buf[(0, 0)].symbol(), "L");
        assert_eq!(buf[(4, 0)].symbol(), "R");
    }
}
