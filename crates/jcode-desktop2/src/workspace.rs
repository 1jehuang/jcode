//! Pure geometry and animation state for the horizontal session workspace.
//!
//! The renderer consumes this module but no GPU or window types leak into it.
//! That keeps the niri-style camera behavior deterministic and makes the edge
//! cases (cyclic session order, two-column ties, interrupted transitions) cheap
//! to exercise in unit tests.

/// Focused columns occupy most, but not all, of the viewport. The remaining
/// strip is split between the adjacent sessions so they are always discoverable.
const COLUMN_FRACTION: f64 = 0.76;
/// Space between session pages, in logical pixels.
pub const GAP: f64 = 14.0;
/// A focus change should be quick enough to feel like navigation, while long
/// enough for the eye to track which neighboring page became active.
pub const TRANSITION_SECONDS: f32 = 0.18;
const PHASE_MAX: u16 = 1000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Direction {
    Left,
    #[default]
    Right,
}

impl Direction {
    fn sign(self) -> f64 {
        match self {
            Self::Left => -1.0,
            Self::Right => 1.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Transition {
    direction: Direction,
    phase: u16,
}

/// Horizontal camera state. Session identity and ordering remain owned by the
/// strip, so switching still uses the existing attach path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Workspace {
    transition: Option<Transition>,
    /// Resolves the exactly-opposite column in an even-sized ring. Retaining the
    /// last direction avoids a column teleporting to the other side when an
    /// animation reaches its final frame.
    side_bias: Direction,
}

impl Default for Workspace {
    fn default() -> Self {
        Self {
            transition: None,
            side_bias: Direction::Right,
        }
    }
}

/// One session page in viewport coordinates. Width is deliberately not scaled:
/// the focused page is native size, while clipping at the viewport exposes its
/// neighbors rather than shrinking the active model into a thumbnail.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Column {
    pub index: usize,
    pub x: f64,
    pub width: f64,
    pub focused: bool,
}

impl Column {
    pub fn is_visible(self, viewport_width: f64) -> bool {
        self.x < viewport_width && self.x + self.width > 0.0
    }
}

impl Workspace {
    /// Start a camera move after the strip has moved focus to its destination.
    /// At phase zero the old neighbor is still centered and the new focused
    /// page starts one pitch away; the offset then eases to zero.
    pub fn begin(&mut self, direction: Direction) {
        self.side_bias = direction;
        self.transition = Some(Transition {
            direction,
            phase: 0,
        });
    }

    pub fn is_animating(&self) -> bool {
        self.transition.is_some()
    }

    /// Advance by elapsed seconds. Returns true when the visible camera changed.
    pub fn advance(&mut self, dt: f32) -> bool {
        let Some(transition) = self.transition.as_mut() else {
            return false;
        };
        let step = (dt.max(0.0) / TRANSITION_SECONDS * f32::from(PHASE_MAX)).max(1.0) as u16;
        transition.phase = transition.phase.saturating_add(step).min(PHASE_MAX);
        if transition.phase == PHASE_MAX {
            self.transition = None;
        }
        true
    }

    fn camera_offset(&self, pitch: f64) -> f64 {
        let Some(transition) = self.transition else {
            return 0.0;
        };
        let linear = f64::from(transition.phase) / f64::from(PHASE_MAX);
        // Smoothstep settles at both ends without a velocity discontinuity.
        let eased = linear * linear * (3.0 - 2.0 * linear);
        transition.direction.sign() * pitch * (1.0 - eased)
    }

    /// Lay out every session around `focused_index` in a cyclic horizontal ring.
    pub fn layout(
        &self,
        session_count: usize,
        focused_index: usize,
        viewport_width: f64,
        column_width: f64,
        gap: f64,
    ) -> Vec<Column> {
        if session_count == 0 {
            return Vec::new();
        }
        let focused_index = focused_index.min(session_count - 1);
        let pitch = column_width + gap;
        let centered = (viewport_width - column_width) / 2.0;
        let camera = self.camera_offset(pitch);
        (0..session_count)
            .map(|index| {
                let relative = ring_offset(index, focused_index, session_count, self.side_bias);
                Column {
                    index,
                    x: centered + relative as f64 * pitch + camera,
                    width: column_width,
                    focused: index == focused_index,
                }
            })
            .collect()
    }
}

/// Native pixel width used to build a session page. One session retains the
/// legacy full-window layout; a workspace reserves enough edge space for both
/// neighboring columns.
pub fn column_width(viewport_width: u32, session_count: usize) -> u32 {
    if session_count <= 1 {
        return viewport_width;
    }
    ((f64::from(viewport_width) * COLUMN_FRACTION).round() as u32).clamp(1, viewport_width.max(1))
}

fn ring_offset(index: usize, focused: usize, len: usize, bias: Direction) -> isize {
    if len <= 1 {
        return 0;
    }
    let forward = (index + len - focused) % len;
    if forward == 0 {
        return 0;
    }
    let backward = forward as isize - len as isize;
    match (forward * 2).cmp(&len) {
        std::cmp::Ordering::Less => forward as isize,
        std::cmp::Ordering::Greater => backward,
        std::cmp::Ordering::Equal => match bias {
            Direction::Left => forward as isize,
            Direction::Right => backward,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIEW: f64 = 1000.0;
    const WIDTH: f64 = 760.0;

    #[test]
    fn focused_column_is_native_width_with_neighbors_visible() {
        let columns = Workspace::default().layout(3, 1, VIEW, WIDTH, GAP);
        let focused = columns.iter().find(|column| column.focused).unwrap();
        assert_eq!(focused.width, WIDTH);
        assert_eq!(focused.x, 120.0);
        assert!(columns[0].is_visible(VIEW));
        assert!(columns[2].is_visible(VIEW));
    }

    #[test]
    fn a_single_session_keeps_the_legacy_full_width() {
        assert_eq!(column_width(1000, 0), 1000);
        assert_eq!(column_width(1000, 1), 1000);
        assert_eq!(column_width(1000, 3), 760);
    }

    #[test]
    fn right_navigation_starts_on_the_previous_page_and_settles_on_target() {
        let mut workspace = Workspace::default();
        workspace.begin(Direction::Right);
        let start = workspace.layout(3, 1, VIEW, WIDTH, GAP);
        assert_eq!(start[0].x, 120.0);
        assert_eq!(start[1].x, 120.0 + WIDTH + GAP);

        workspace.advance(TRANSITION_SECONDS / 2.0);
        let middle = workspace.layout(3, 1, VIEW, WIDTH, GAP);
        assert!(middle[1].x > 120.0);
        assert!(middle[1].x < start[1].x);

        workspace.advance(TRANSITION_SECONDS);
        let end = workspace.layout(3, 1, VIEW, WIDTH, GAP);
        assert!(!workspace.is_animating());
        assert!((end[1].x - 120.0).abs() < f64::EPSILON);
    }

    #[test]
    fn left_navigation_is_the_mirror_image() {
        let mut workspace = Workspace::default();
        workspace.begin(Direction::Left);
        let columns = workspace.layout(3, 1, VIEW, WIDTH, GAP);
        assert_eq!(columns[2].x, 120.0);
        assert_eq!(columns[1].x, 120.0 - WIDTH - GAP);
    }

    #[test]
    fn two_session_tie_stays_on_the_transition_side_after_settling() {
        let mut workspace = Workspace::default();
        workspace.begin(Direction::Right);
        workspace.advance(TRANSITION_SECONDS * 2.0);
        let columns = workspace.layout(2, 1, VIEW, WIDTH, GAP);
        assert!(columns[0].x < columns[1].x);

        workspace.begin(Direction::Left);
        workspace.advance(TRANSITION_SECONDS * 2.0);
        let columns = workspace.layout(2, 1, VIEW, WIDTH, GAP);
        assert!(columns[0].x > columns[1].x);
    }

    #[test]
    fn invisible_columns_are_still_stably_ordered_in_the_ring() {
        let columns = Workspace::default().layout(7, 3, VIEW, WIDTH, GAP);
        assert_eq!(columns.len(), 7);
        assert_eq!(columns.iter().filter(|column| column.focused).count(), 1);
        assert_eq!(columns[3].x, 120.0);
        assert!(columns[2].x < columns[3].x);
        assert!(columns[4].x > columns[3].x);
    }
}
