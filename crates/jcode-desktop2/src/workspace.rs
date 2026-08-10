//! Pure geometry and animation state for the niri-style session workspace.
//!
//! The renderer consumes this module but no GPU or window types leak into it.
//! That keeps the camera behavior deterministic and makes the edge cases
//! (cyclic session order, two-column ties, interrupted transitions, mid-slide
//! row changes) cheap to exercise in unit tests.
//!
//! The spatial model is the compositor's own: a *row* is a working directory
//! (a niri workspace) and a *column* is a session in it (a window). Left and
//! right slide the camera along the focused row; up and down slide the whole
//! row off and the next one in, exactly the motion niri makes for a workspace
//! switch. Only the focused row is ever on screen when the camera is at rest,
//! which is what makes a column's neighbors always mean "same project".

/// Focused columns occupy most, but not all, of the viewport. The remaining
/// strip is split between the adjacent sessions so they are always
/// discoverable.
const COLUMN_FRACTION: f64 = 0.76;
const COLUMN_FRACTION_UNITS: u16 = 760;
const MIN_COLUMN_FRACTION_UNITS: u16 = 400;
const MAX_COLUMN_FRACTION_UNITS: u16 = 1000;
const COLUMN_RESIZE_STEP_UNITS: u16 = 50;
/// Space between session pages, in logical pixels.
pub const GAP: f64 = 14.0;
/// Breathing room above and below every page while the workspace chrome is
/// drawn, in logical pixels. This is what turns "one full-bleed page" into
/// "windows on a desk": without it the boundary rings would run into the
/// window edge and read as clutter rather than as borders.
pub const VERTICAL_INSET: f64 = 10.0;
/// A focus change should be quick enough to feel like navigation, while long
/// enough for the eye to track which neighboring page became active.
pub const TRANSITION_SECONDS: f32 = 0.18;
/// A row change travels the full window height, so it gets slightly longer
/// than a column slide or the motion reads as a flash rather than a move.
pub const ROW_TRANSITION_SECONDS: f32 = 0.22;
const PHASE_MAX: u16 = 1000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Direction {
    Left,
    #[default]
    Right,
    Up,
    Down,
}

impl Direction {
    fn sign(self) -> f64 {
        match self {
            Self::Left | Self::Up => -1.0,
            Self::Right | Self::Down => 1.0,
        }
    }

    pub fn is_horizontal(self) -> bool {
        matches!(self, Self::Left | Self::Right)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Transition {
    direction: Direction,
    phase: u16,
    /// Stored per transition because a column slide and a row slide run at
    /// different speeds. Milliseconds, so the model stays `Eq`.
    duration_ms: u16,
}

/// Horizontal and vertical camera state. Session identity and ordering remain
/// owned by the strip, so switching still uses the existing attach path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Workspace {
    transition: Option<Transition>,
    /// Resolves the exactly-opposite column in an even-sized ring. Retaining
    /// the last direction avoids a column teleporting to the other side when
    /// an animation reaches its final frame.
    side_bias: Direction,
    /// The sessions of the row being left, captured when a vertical slide
    /// begins. Ids rather than indices because the session list can be
    /// re-polled mid-slide, and an index into a reshuffled list would draw
    /// the wrong conversations flying off screen.
    prev_row: Vec<String>,
    /// Which column of the departing row was focused, so it exits centered.
    prev_focused: usize,
    /// Width of the focused session page in thousandths of the viewport.
    /// This changes the child page only; the containing OS window is untouched.
    column_fraction: u16,
}

impl Default for Workspace {
    fn default() -> Self {
        Self {
            transition: None,
            side_bias: Direction::Right,
            prev_row: Vec::new(),
            prev_focused: 0,
            column_fraction: COLUMN_FRACTION_UNITS,
        }
    }
}

/// One row of sessions, as the camera needs to know it: which flat entries it
/// holds, which of them is focused, and how wide its columns are.
#[derive(Clone, Debug, PartialEq)]
pub struct RowSpec {
    /// Indices into the strip's flat entry list, in row order.
    pub indices: Vec<usize>,
    /// Position of the focused column within `indices`.
    pub focused_pos: usize,
    /// Native pixel width of each column in this row.
    pub column_width: f64,
}

/// One session page in viewport coordinates. Width is deliberately not scaled:
/// the focused page is native size, while clipping at the viewport exposes its
/// neighbors rather than shrinking the active model into a thumbnail.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Column {
    pub index: usize,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub focused: bool,
}

impl Column {
    /// Whether any part of this full-height page intersects the viewport.
    pub fn is_visible(self, viewport: (f64, f64)) -> bool {
        self.x < viewport.0
            && self.x + self.width > 0.0
            && self.y < viewport.1
            && self.y + viewport.1 > 0.0
    }
}

impl Workspace {
    /// Grow or shrink the focused session page. Returns whether it changed.
    pub fn resize_column(&mut self, grow: bool) -> bool {
        let previous = self.column_fraction;
        self.column_fraction = if grow {
            self.column_fraction
                .saturating_add(COLUMN_RESIZE_STEP_UNITS)
        } else {
            self.column_fraction
                .saturating_sub(COLUMN_RESIZE_STEP_UNITS)
        }
        .clamp(MIN_COLUMN_FRACTION_UNITS, MAX_COLUMN_FRACTION_UNITS);
        self.column_fraction != previous
    }

    pub fn column_percent(&self) -> u16 {
        self.column_fraction / 10
    }

    pub fn column_width(&self, viewport_width: u32, session_count: usize) -> u32 {
        if session_count <= 1 {
            return viewport_width;
        }
        ((u64::from(viewport_width) * u64::from(self.column_fraction) + 500) / 1000) as u32
    }

    /// Start a horizontal camera move after the strip has moved focus to its
    /// destination. At phase zero the old neighbor is still centered and the
    /// new focused page starts one pitch away; the offset then eases to zero.
    pub fn begin(&mut self, direction: Direction) {
        if !direction.is_horizontal() {
            return;
        }
        self.side_bias = direction;
        self.prev_row.clear();
        self.transition = Some(Transition {
            direction,
            phase: 0,
            duration_ms: (TRANSITION_SECONDS * 1000.0) as u16,
        });
    }

    /// Start a vertical row slide after the strip has moved focus to another
    /// group. `prev_row` is the departing row's session ids and `prev_focused`
    /// which of them was centered, so the old workspace exits exactly as it
    /// stood rather than snapping to some canonical arrangement first.
    pub fn begin_row_change(
        &mut self,
        direction: Direction,
        prev_row: Vec<String>,
        prev_focused: usize,
    ) {
        if direction.is_horizontal() {
            self.begin(direction);
            return;
        }
        self.prev_row = prev_row;
        self.prev_focused = prev_focused;
        self.transition = Some(Transition {
            direction,
            phase: 0,
            duration_ms: (ROW_TRANSITION_SECONDS * 1000.0) as u16,
        });
    }

    pub fn is_animating(&self) -> bool {
        self.transition.is_some()
    }

    /// The running vertical slide's direction, if one is running. The scene
    /// uses this to know the departing row still needs drawing.
    pub fn row_change(&self) -> Option<Direction> {
        self.transition
            .filter(|transition| !transition.direction.is_horizontal())
            .map(|transition| transition.direction)
    }

    /// The departing row: its session ids and its focused position.
    pub fn prev_row(&self) -> (&[String], usize) {
        (&self.prev_row, self.prev_focused)
    }

    /// Advance by elapsed seconds. Returns true when the visible camera changed.
    pub fn advance(&mut self, dt: f32) -> bool {
        let Some(transition) = self.transition.as_mut() else {
            return false;
        };
        let seconds = (f32::from(transition.duration_ms) / 1000.0).max(0.01);
        let step = (dt.max(0.0) / seconds * f32::from(PHASE_MAX)).max(1.0) as u16;
        transition.phase = transition.phase.saturating_add(step).min(PHASE_MAX);
        if transition.phase == PHASE_MAX {
            self.transition = None;
            self.prev_row.clear();
        }
        true
    }

    /// Eased progress of the running transition, 1.0 at rest.
    fn progress(&self) -> f64 {
        let Some(transition) = self.transition else {
            return 1.0;
        };
        let linear = f64::from(transition.phase) / f64::from(PHASE_MAX);
        // Smoothstep settles at both ends without a velocity discontinuity.
        linear * linear * (3.0 - 2.0 * linear)
    }

    /// Lay out the focused row (and, during a vertical slide, the departing
    /// one) in viewport coordinates.
    pub fn layout(
        &self,
        current: &RowSpec,
        previous: Option<&RowSpec>,
        viewport: (f64, f64),
        gap: f64,
    ) -> Vec<Column> {
        let eased = self.progress();
        match self.transition {
            Some(transition) if !transition.direction.is_horizontal() => {
                // The new row arrives from the direction the user moved (down
                // brings the row below up into place) while the old one exits
                // out the opposite edge, one full viewport height apart.
                let travel = transition.direction.sign() * viewport.1;
                let dy = travel * (1.0 - eased);
                let mut columns =
                    row_columns(current, viewport, gap, 0.0, dy, self.side_bias, true);
                if let Some(previous) = previous {
                    columns.extend(row_columns(
                        previous,
                        viewport,
                        gap,
                        0.0,
                        dy - travel,
                        self.side_bias,
                        false,
                    ));
                }
                columns
            }
            Some(transition) => {
                let pitch = current.column_width + gap;
                let dx = transition.direction.sign() * pitch * (1.0 - eased);
                row_columns(current, viewport, gap, dx, 0.0, self.side_bias, true)
            }
            None => row_columns(current, viewport, gap, 0.0, 0.0, self.side_bias, true),
        }
    }
}

/// Lay one row out as a cyclic horizontal ring around its focused column.
fn row_columns(
    row: &RowSpec,
    viewport: (f64, f64),
    gap: f64,
    dx: f64,
    dy: f64,
    bias: Direction,
    carries_focus: bool,
) -> Vec<Column> {
    if row.indices.is_empty() {
        return Vec::new();
    }
    let len = row.indices.len();
    let focused_pos = row.focused_pos.min(len - 1);
    let pitch = row.column_width + gap;
    let centered = (viewport.0 - row.column_width) / 2.0;
    row.indices
        .iter()
        .enumerate()
        .map(|(pos, &index)| {
            let relative = ring_offset(pos, focused_pos, len, bias);
            Column {
                index,
                x: centered + relative as f64 * pitch + dx,
                y: dy,
                width: row.column_width,
                focused: carries_focus && pos == focused_pos,
            }
        })
        .collect()
}

/// Native pixel width used to build a session page. A row of one session
/// keeps the legacy full-window layout; a wider row reserves enough edge
/// space for both neighboring columns.
pub fn column_width(viewport_width: u32, session_count: usize) -> u32 {
    if session_count <= 1 {
        return viewport_width;
    }
    ((f64::from(viewport_width) * COLUMN_FRACTION).round() as u32).clamp(1, viewport_width.max(1))
}

/// Resolve the strip into camera columns: the focused working-dir group as
/// the current row, plus the departing group while a vertical slide runs.
///
/// One function shared by the renderer and by pointer conversion, for the
/// same reason [`crate::layout::Frame`] is shared: if the two ever disagreed,
/// clicks would land on a different page than the one under the cursor.
pub fn placement(
    strip: &crate::strip::Strips,
    workspace: &Workspace,
    session_id: Option<&str>,
    viewport: (f64, f64),
    gap: f64,
) -> Vec<Column> {
    let groups = strip.strips();
    if groups.is_empty() {
        return Vec::new();
    }
    // Flat entry index of each group's first session, matching
    // `Strip::entries` order, so a `Column::index` addresses the same session
    // everywhere.
    let mut bases = Vec::with_capacity(groups.len());
    let mut base = 0usize;
    for group in groups {
        bases.push(base);
        base += group.panels.len();
    }
    let locate = |id: &str| {
        groups.iter().enumerate().find_map(|(g, group)| {
            group
                .panels
                .iter()
                .position(|entry| entry.session_id == id)
                .map(|i| (g, i))
        })
    };
    // The attached session anchors the camera; the strip's own focus is the
    // fallback for the moments before an attach resolves.
    let (group, pos) = session_id.and_then(locate).unwrap_or_else(|| {
        let group = strip.strip_index().min(groups.len() - 1);
        let len = groups[group].panels.len();
        (group, strip.panel_index().min(len.saturating_sub(1)))
    });
    let row: Vec<usize> = (0..groups[group].panels.len())
        .map(|i| bases[group] + i)
        .collect();
    let current = RowSpec {
        column_width: f64::from(workspace.column_width(viewport.0.round() as u32, row.len())),
        focused_pos: pos,
        indices: row,
    };
    let previous = workspace
        .row_change()
        .map(|_| {
            let (ids, prev_focused) = workspace.prev_row();
            let indices: Vec<usize> = ids
                .iter()
                .filter_map(|id| locate(id).map(|(g, i)| bases[g] + i))
                .collect();
            RowSpec {
                column_width: f64::from(
                    workspace.column_width(viewport.0.round() as u32, indices.len()),
                ),
                focused_pos: prev_focused.min(indices.len().saturating_sub(1)),
                indices,
            }
        })
        .filter(|row| !row.indices.is_empty());
    workspace.layout(&current, previous.as_ref(), viewport, gap)
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
            _ => backward,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strip::{Panel, Strips};

    const VIEW: (f64, f64) = (1000.0, 700.0);
    const WIDTH: f64 = 760.0;

    fn row(count: usize, focused_pos: usize) -> RowSpec {
        RowSpec {
            indices: (0..count).collect(),
            focused_pos,
            column_width: WIDTH,
        }
    }

    #[test]
    fn focused_column_is_native_width_with_neighbors_visible() {
        let columns = Workspace::default().layout(&row(3, 1), None, VIEW, GAP);
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
    fn resized_panel_width_survives_focus_transitions() {
        let mut workspace = Workspace::default();
        assert!(workspace.resize_column(true));
        let resized = workspace.column_width(1000, 3);

        workspace.begin(Direction::Right);
        assert_eq!(workspace.column_width(1000, 3), resized);
        workspace.advance(TRANSITION_SECONDS);
        assert_eq!(workspace.column_width(1000, 3), resized);

        workspace.begin_row_change(Direction::Down, vec!["old".into()], 0);
        assert_eq!(workspace.column_width(1000, 3), resized);
    }

    #[test]
    fn right_navigation_starts_on_the_previous_page_and_settles_on_target() {
        let mut workspace = Workspace::default();
        workspace.begin(Direction::Right);
        let start = workspace.layout(&row(3, 1), None, VIEW, GAP);
        assert_eq!(start[0].x, 120.0);
        assert_eq!(start[1].x, 120.0 + WIDTH + GAP);

        workspace.advance(TRANSITION_SECONDS / 2.0);
        let middle = workspace.layout(&row(3, 1), None, VIEW, GAP);
        assert!(middle[1].x > 120.0);
        assert!(middle[1].x < start[1].x);

        workspace.advance(TRANSITION_SECONDS);
        let end = workspace.layout(&row(3, 1), None, VIEW, GAP);
        assert!(!workspace.is_animating());
        assert!((end[1].x - 120.0).abs() < f64::EPSILON);
    }

    #[test]
    fn left_navigation_is_the_mirror_image() {
        let mut workspace = Workspace::default();
        workspace.begin(Direction::Left);
        let columns = workspace.layout(&row(3, 1), None, VIEW, GAP);
        assert_eq!(columns[2].x, 120.0);
        assert_eq!(columns[1].x, 120.0 - WIDTH - GAP);
    }

    #[test]
    fn two_session_tie_stays_on_the_transition_side_after_settling() {
        let mut workspace = Workspace::default();
        workspace.begin(Direction::Right);
        workspace.advance(TRANSITION_SECONDS * 2.0);
        let columns = workspace.layout(&row(2, 1), None, VIEW, GAP);
        assert!(columns[0].x < columns[1].x);

        workspace.begin(Direction::Left);
        workspace.advance(TRANSITION_SECONDS * 2.0);
        let columns = workspace.layout(&row(2, 1), None, VIEW, GAP);
        assert!(columns[0].x > columns[1].x);
    }

    #[test]
    fn invisible_columns_are_still_stably_ordered_in_the_ring() {
        let columns = Workspace::default().layout(&row(7, 3), None, VIEW, GAP);
        assert_eq!(columns.len(), 7);
        assert_eq!(columns.iter().filter(|column| column.focused).count(), 1);
        assert_eq!(columns[3].x, 120.0);
        assert!(columns[2].x < columns[3].x);
        assert!(columns[4].x > columns[3].x);
    }

    /// The niri workspace switch: moving down brings the new row up from the
    /// bottom while the old one exits out the top, and the slide settles with
    /// only the new row on screen.
    #[test]
    fn a_downward_row_change_slides_the_old_row_out_the_top() {
        let mut workspace = Workspace::default();
        workspace.begin_row_change(Direction::Down, vec!["old".into()], 0);
        let prev = RowSpec {
            indices: vec![9],
            focused_pos: 0,
            column_width: VIEW.0,
        };

        let start = workspace.layout(&row(2, 0), Some(&prev), VIEW, GAP);
        let new_row: Vec<&Column> = start.iter().filter(|c| c.index != 9).collect();
        let old = start.iter().find(|c| c.index == 9).unwrap();
        assert_eq!(old.y, 0.0, "the departing row did not start centered");
        assert!(
            new_row.iter().all(|c| (c.y - VIEW.1).abs() < 1e-9),
            "the arriving row did not start one viewport below"
        );
        assert!(!old.focused, "the departing row kept focus");
        assert_eq!(new_row.iter().filter(|c| c.focused).count(), 1);

        workspace.advance(ROW_TRANSITION_SECONDS / 2.0);
        let middle = workspace.layout(&row(2, 0), Some(&prev), VIEW, GAP);
        let old = middle.iter().find(|c| c.index == 9).unwrap();
        assert!(old.y < 0.0, "the departing row never started leaving");

        workspace.advance(ROW_TRANSITION_SECONDS * 2.0);
        assert!(!workspace.is_animating());
        assert_eq!(workspace.row_change(), None);
        let end = workspace.layout(&row(2, 0), None, VIEW, GAP);
        assert!(end.iter().all(|c| c.y == 0.0));
    }

    #[test]
    fn an_upward_row_change_is_the_mirror_image() {
        let mut workspace = Workspace::default();
        workspace.begin_row_change(Direction::Up, vec!["old".into()], 0);
        let prev = RowSpec {
            indices: vec![9],
            focused_pos: 0,
            column_width: VIEW.0,
        };
        let start = workspace.layout(&row(1, 0), Some(&prev), VIEW, GAP);
        let arriving = start.iter().find(|c| c.index == 0).unwrap();
        assert!(
            (arriving.y + VIEW.1).abs() < 1e-9,
            "up did not arrive from above"
        );
    }

    fn entry(id: &str, dir: &str) -> Panel {
        Panel {
            session_id: id.into(),
            title: None,
            working_dir: Some(dir.into()),
            busy: false,
            weight: 0.0,
        }
    }

    /// Only the focused working directory's sessions are on screen at rest:
    /// that is what makes a column's neighbors always mean "same project".
    #[test]
    fn placement_shows_only_the_focused_group_at_rest() {
        let strip = Strips::build(
            vec![
                entry("a1", "/w/jcode"),
                entry("a2", "/w/jcode"),
                entry("b1", "/w/site"),
            ],
            Some("a1"),
        );
        let columns = placement(&strip, &Workspace::default(), Some("a1"), VIEW, GAP);
        let indices: Vec<usize> = columns.iter().map(|c| c.index).collect();
        assert_eq!(indices, vec![0, 1], "another project's session leaked in");
        assert!(columns[0].focused);
    }

    /// During a vertical slide both rows exist, each with its own column
    /// width, and the departing row is resolved by session id so a re-polled
    /// list cannot make the wrong pages fly off.
    #[test]
    fn placement_draws_the_departing_row_during_a_slide() {
        let strip = Strips::build(
            vec![
                entry("a1", "/w/jcode"),
                entry("a2", "/w/jcode"),
                entry("b1", "/w/site"),
            ],
            Some("b1"),
        );
        let mut workspace = Workspace::default();
        workspace.begin_row_change(Direction::Down, vec!["a1".into(), "a2".into()], 1);
        let columns = placement(&strip, &workspace, Some("b1"), VIEW, GAP);
        let indices: Vec<usize> = columns.iter().map(|c| c.index).collect();
        assert_eq!(indices, vec![2, 0, 1]);
        // The lone arriving column is full width; the departing pair is not.
        assert_eq!(columns[0].width, VIEW.0);
        assert_eq!(columns[1].width, WIDTH);
        // The departing row exits as it stood: its second column centered.
        let a2 = columns.iter().find(|c| c.index == 1).unwrap();
        assert_eq!(a2.x, 120.0);
        assert!(!a2.focused);
    }

    /// A departed session that vanished from a re-poll is simply skipped
    /// rather than panicking or drawing a stranger in its place.
    #[test]
    fn placement_survives_a_departing_session_disappearing() {
        let strip = Strips::build(vec![entry("b1", "/w/site")], Some("b1"));
        let mut workspace = Workspace::default();
        workspace.begin_row_change(Direction::Down, vec!["gone".into()], 0);
        let columns = placement(&strip, &workspace, Some("b1"), VIEW, GAP);
        assert_eq!(columns.len(), 1);
        assert_eq!(columns[0].index, 0);
    }
}
