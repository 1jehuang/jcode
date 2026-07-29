//! The session overview: every live session as a card in a niri-style strip.
//!
//! Held Super zooms the window out of the conversation you are in and into
//! the space of all of them. The layout is the compositor's own overview,
//! which is the motion the author already has in his fingers: a *row* is a
//! working directory (a workspace) and a *card* is a session in it (a
//! window), so "where am I, and what else is running" reads the same way it
//! does on the desktop. A card's width tracks how much conversation the
//! session holds, so "the wide one in the jcode row" is a thing you can point
//! at and remember.
//!
//! Placement is deterministic: rows stack in first-appearance order and cards
//! run left to right in theirs. No randomness and no clock, so the same
//! session set always lays out identically. That is what stops the field from
//! reshuffling under the user's fingers between polls, and what lets the
//! whole thing be tested without a GPU.
//!
//! This module is pure. It owns the sizing, the placement, the focus, and the
//! directional navigation; the renderer and the app only consume it.

use crate::strip::Entry;

/// Narrowest a card may be drawn, in logical units. A session with no
/// conversation yet is still a target you have to be able to see and click.
const MIN_CARD_WIDTH: f64 = 96.0;
/// Widest a card may be drawn. Capped so one enormous session cannot push the
/// rest of its row off the page: the overview is for comparing sessions, not
/// for rendering a truthful bar chart.
const MAX_CARD_WIDTH: f64 = 280.0;
/// Gap between two cards in a row. Tight, like niri's window gaps: sessions
/// in one project should read as one workspace, not as scattered tiles.
const CARD_GAP: f64 = 10.0;
/// Gap between two rows. Wider than [`CARD_GAP`] so two projects read as two
/// places: what separates them is the *contrast* between the two spacings.
const ROW_GAP: f64 = 30.0;
/// Tallest a row of cards may be. Rows share the field's height evenly and
/// clamp here, so one lone project is a comfortable band rather than a wall.
const MAX_ROW_HEIGHT: f64 = 132.0;
/// Room reserved above each row for its label, in logical units.
pub const ROW_LABEL_BAND: f64 = 20.0;

/// The region the field is laid out in: the page inside its margins.
///
/// One definition shared by the renderer and by pointer hit-testing, for the
/// same reason [`crate::layout::Frame`] is: if the two ever disagreed, clicks
/// would land on a different card than the one under the cursor.
pub fn area(frame: &crate::layout::Frame) -> (f64, f64, f64, f64) {
    let inset = (frame.width * 0.04).clamp(16.0, 56.0);
    // The overview replaces the page rather than sitting in the transcript's
    // slot, so it gets the window from the top margin down to the hint row.
    // Anchoring the top at `body_top` left the field crowded into the lower
    // two thirds with a band of blank paper above it.
    let bottom = frame.height - crate::layout::FOOTNOTE_HEIGHT * 2.5;
    (inset, inset, frame.width - inset, bottom.max(inset + 1.0))
}

/// A direction for keyboard navigation across the field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

/// One placed session.
#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    /// Index into the entry list the field was built from.
    pub index: usize,
    pub session_id: String,
    /// Label of the row this card belongs to: the working directory's leaf.
    pub label: String,
    /// The card's drawn bounds, `(x0, y0, x1, y1)` in logical units.
    pub rect: (f64, f64, f64, f64),
    pub busy: bool,
    pub focused: bool,
    /// Whether this is the session the window is currently attached to, which
    /// is drawn as the one you zoomed out of.
    pub current: bool,
}

impl Card {
    pub fn center(&self) -> (f64, f64) {
        (
            (self.rect.0 + self.rect.2) / 2.0,
            (self.rect.1 + self.rect.3) / 2.0,
        )
    }
}

/// A row's label anchor: the band reserved above its cards, so the name sits
/// with the workspace rather than at a grid position of its own.
#[derive(Clone, Debug, PartialEq)]
pub struct RowLabel {
    pub label: String,
    /// Left edge of the row's first card, which the label aligns to.
    pub left: f64,
    /// Baseline-ish top for the label, inside the reserved band.
    pub top: f64,
}

/// A laid-out field of cards.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Field {
    pub cards: Vec<Card>,
    pub rows: Vec<RowLabel>,
    /// Card indices per row, in left-to-right order. This is the navigation
    /// structure: left/right walks a row, up/down walks between them.
    members: Vec<Vec<usize>>,
}

impl Field {
    pub fn focused(&self) -> Option<&Card> {
        self.cards.iter().find(|card| card.focused)
    }

    /// The card under a logical point, if any.
    pub fn hit(&self, x: f64, y: f64) -> Option<&Card> {
        self.cards.iter().find(|card| {
            let (x0, y0, x1, y1) = card.rect;
            x >= x0 && x <= x1 && y >= y0 && y <= y1
        })
    }

    /// Row and position of a session, if the field has it.
    fn locate(&self, session_id: &str) -> Option<(usize, usize)> {
        for (row, members) in self.members.iter().enumerate() {
            if let Some(at) = members
                .iter()
                .position(|index| self.cards[*index].session_id == session_id)
            {
                return Some((row, at));
            }
        }
        None
    }

    /// The session a directional move from `from` should land on.
    ///
    /// Left and right walk the row and *stop at its ends*: wrapping made a
    /// keystroke at the edge teleport to the far side of the row, which reads
    /// as the highlight jumping rather than moving. Up and down move between
    /// rows, also clamped at the top and bottom, landing on the card whose
    /// centre is nearest the one you left, which is the compositor's own
    /// column-preserving motion.
    pub fn neighbor(&self, from: &str, dir: Dir) -> Option<&Card> {
        let (row, at) = self.locate(from).or_else(|| {
            self.cards
                .first()
                .and_then(|card| self.locate(&card.session_id))
        })?;
        let members = &self.members[row];
        match dir {
            Dir::Left => {
                let next = at.checked_sub(1)?;
                Some(&self.cards[members[next]])
            }
            Dir::Right => {
                let next = at + 1;
                if next >= members.len() {
                    return None;
                }
                Some(&self.cards[members[next]])
            }
            Dir::Up | Dir::Down => {
                let target = if dir == Dir::Down {
                    let below = row + 1;
                    if below >= self.members.len() {
                        return None;
                    }
                    below
                } else {
                    row.checked_sub(1)?
                };
                let from_x = self.cards[members[at]].center().0;
                self.members[target]
                    .iter()
                    .map(|index| &self.cards[*index])
                    .min_by(|a, b| {
                        (a.center().0 - from_x)
                            .abs()
                            .total_cmp(&(b.center().0 - from_x).abs())
                    })
            }
        }
    }

    /// Whether a directional move from `from` could never go anywhere in this
    /// field, as opposed to merely being at an edge right now: a row of one
    /// makes left/right dead, and a single row makes up/down dead. The caller
    /// uses this to tell "broken axis, fall back to something useful" apart
    /// from "edge of the field, stay put".
    pub fn axis_is_dead(&self, from: &str, dir: Dir) -> bool {
        let Some((row, _)) = self.locate(from).or_else(|| {
            self.cards
                .first()
                .and_then(|card| self.locate(&card.session_id))
        }) else {
            return true;
        };
        match dir {
            Dir::Left | Dir::Right => self.members[row].len() <= 1,
            Dir::Up | Dir::Down => self.members.len() <= 1,
        }
    }

    /// Session ids in reading order (left to right, top to bottom), for the
    /// Tab cycle. Row order is layout order, so Tab walks the field the way
    /// the eye does.
    pub fn reading_order(&self) -> Vec<&str> {
        self.members
            .iter()
            .flat_map(|members| {
                members
                    .iter()
                    .map(|index| self.cards[*index].session_id.as_str())
            })
            .collect()
    }

    /// The next session after `from` in reading order, wrapping.
    pub fn next_in_order(&self, from: &str, step: isize) -> Option<&str> {
        let order = self.reading_order();
        if order.is_empty() {
            return None;
        }
        let at = order.iter().position(|id| *id == from).unwrap_or(0) as isize;
        let next = (at + step).rem_euclid(order.len() as isize) as usize;
        order.get(next).copied()
    }
}

/// Card width for a session's weight.
///
/// `sqrt` of a normalized weight, so the heaviest session in the field sets
/// the top of the scale and the difference between a long conversation and a
/// fresh one stays legible without the long one being drawn ten times as
/// wide.
fn width_for(weight: f64, heaviest: f64) -> f64 {
    if heaviest <= 0.0 {
        return MIN_CARD_WIDTH;
    }
    let normalized = (weight.max(0.0) / heaviest).clamp(0.0, 1.0);
    MIN_CARD_WIDTH + (MAX_CARD_WIDTH - MIN_CARD_WIDTH) * normalized.sqrt()
}

/// Group entries by working-directory leaf, preserving first-appearance order.
///
/// Same rule as the strip, and for the same reason: a field whose rows
/// reorder between polls is unreadable.
fn row_of(entry: &Entry) -> String {
    let Some(dir) = entry.working_dir.as_deref() else {
        return "-".to_string();
    };
    let trimmed = dir.trim_end_matches('/');
    match trimmed.rsplit('/').next() {
        Some(leaf) if !leaf.is_empty() => leaf.to_string(),
        _ => "/".to_string(),
    }
}

/// Lay out the field inside `area`.
///
/// `focus` is the highlighted session and `current` the one the window is
/// attached to; they differ while the user is moving around the field before
/// committing.
pub fn layout(
    entries: &[Entry],
    focus: Option<&str>,
    current: Option<&str>,
    area: (f64, f64, f64, f64),
) -> Field {
    if entries.is_empty() {
        return Field::default();
    }
    let heaviest = entries
        .iter()
        .map(|entry| entry.weight)
        .fold(0.0f64, f64::max);

    // Bucket into rows, keeping first-appearance order.
    let mut labels: Vec<String> = Vec::new();
    let mut members: Vec<Vec<usize>> = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let label = row_of(entry);
        match labels.iter().position(|name| *name == label) {
            Some(at) => members[at].push(index),
            None => {
                labels.push(label);
                members.push(vec![index]);
            }
        }
    }

    let (x0, y0, x1, y1) = area;
    let field_width = (x1 - x0).max(1.0);
    let field_height = (y1 - y0).max(1.0);

    // Rows share the height evenly, clamped to a comfortable band. The block
    // is then centred vertically, so one project sits in the middle of the
    // page rather than hanging off the top.
    let rows = members.len() as f64;
    let available = field_height - (rows - 1.0) * ROW_GAP - rows * ROW_LABEL_BAND;
    let row_height = (available / rows).clamp(12.0, MAX_ROW_HEIGHT);
    let block = rows * (row_height + ROW_LABEL_BAND) + (rows - 1.0) * ROW_GAP;
    let mut top = y0 + ((field_height - block) / 2.0).max(0.0);

    let mut cards: Vec<Option<Card>> = vec![None; entries.len()];
    let mut row_labels: Vec<RowLabel> = Vec::with_capacity(members.len());
    for (row, group) in members.iter().enumerate() {
        // Natural widths, then one uniform squeeze when the row cannot fit:
        // the *relative* widths (the point of the sizing) survive, and the
        // row never runs off the page the way the compositor's strip can.
        let widths: Vec<f64> = group
            .iter()
            .map(|index| width_for(entries[*index].weight, heaviest))
            .collect();
        let natural: f64 =
            widths.iter().sum::<f64>() + (group.len() as f64 - 1.0).max(0.0) * CARD_GAP;
        let gaps = (group.len() as f64 - 1.0).max(0.0) * CARD_GAP;
        let squeeze = if natural > field_width {
            ((field_width - gaps) / (natural - gaps)).max(0.05)
        } else {
            1.0
        };
        let row_width = (natural - gaps) * squeeze + gaps;
        // Centred, like the compositor centres the focused column: the rows
        // read as a stack of workspaces rather than as a table flushed left.
        let mut x = x0 + (field_width - row_width) / 2.0;
        let label_top = top;
        let cards_top = top + ROW_LABEL_BAND;
        row_labels.push(RowLabel {
            label: labels[row].clone(),
            left: x,
            top: label_top,
        });
        for (rank, index) in group.iter().enumerate() {
            let width = widths[rank] * squeeze;
            let entry = &entries[*index];
            cards[*index] = Some(Card {
                index: *index,
                session_id: entry.session_id.clone(),
                label: labels[row].clone(),
                rect: (x, cards_top, x + width, cards_top + row_height),
                busy: entry.busy,
                focused: focus == Some(entry.session_id.as_str()),
                current: current == Some(entry.session_id.as_str()),
            });
            x += width + CARD_GAP;
        }
        top = cards_top + row_height + ROW_GAP;
    }

    Field {
        cards: cards.into_iter().flatten().collect(),
        rows: row_labels,
        members,
    }
}

/// A card's caption: the human-readable part of a session id.
///
/// Session ids are `session_<name>_<millis>_<hash>`, of which only the name is
/// worth reading. Printing the whole id would make every card look identical
/// at a glance, which is the one thing the field must not do.
///
/// The *first* all-alphabetic segment wins, which is the daemon's generated
/// name. Preferring the longest instead picked the trailing hex hash, and a
/// field of eighteen identical-looking hashes is exactly the failure the label
/// exists to prevent. Ids that carry no such segment fall back to a prefix,
/// which is at least distinguishing.
pub fn short_id(session_id: &str) -> String {
    let trimmed = session_id.strip_prefix("session_").unwrap_or(session_id);
    trimmed
        .split(['_', '-'])
        .find(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_alphabetic()))
        .map(|part| part.chars().take(12).collect())
        .unwrap_or_else(|| trimmed.chars().take(8).collect())
}

/// A 0..1 breath over `elapsed` time, for the busy pulse.
///
/// Lives here rather than in the renderer so the sizing of a card is one
/// function of (card, elapsed) that a test can evaluate without a GPU.
///
/// Driven by elapsed time rather than the wall clock so a pinned capture is
/// byte-reproducible: the caller passes [`crate::activity::Activity::elapsed`],
/// which state-space nodes pin exactly as they pin the caret and the stream.
/// This also matches the frame scheduling: the loop only wakes for the
/// spinner while a turn runs, which is exactly when this clock advances.
pub fn breath(elapsed: std::time::Duration, period: f32) -> f64 {
    let turn = std::f32::consts::TAU * elapsed.as_secs_f32() / period.max(0.01);
    f64::from(turn.sin())
}

/// How long the zoom takes, in seconds. This is a flick gesture, so the
/// animation exists only to show *where* the field came from: any longer and
/// it stops being a shortcut and becomes a menu you have to sit through.
pub const ZOOM: f32 = 0.08;

/// The overview's state: whether it is showing, how far the zoom has got, and
/// which session the user is pointing at.
///
/// Held in the model rather than in the app so a frame stays a pure function
/// of the model and every phase of the zoom can be captured in a pixel test.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Overview {
    /// Whether the user is currently holding the overview open.
    open: bool,
    /// Zoom progress in thousandths, 0 closed to 1000 fully open. An integer
    /// so the model stays `Eq` and captures can pin an exact mid-zoom frame.
    phase: u16,
    /// The highlighted session. `None` falls back to the attached one.
    focus: Option<String>,
}

const PHASE_MAX: u16 = 1000;

/// Fetched tails of other sessions, for the preview behind the field.
///
/// Kept as its own type rather than a bare map so the "asked but not yet
/// answered" state is explicit: without it, hovering a slow session would
/// re-request its tail on every frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Peeks {
    fetched: std::collections::HashMap<String, crate::transcript::Transcript>,
    requested: std::collections::BTreeSet<String>,
}

impl Peeks {
    /// Record a fetched tail.
    pub fn insert(&mut self, session_id: &str, transcript: crate::transcript::Transcript) {
        self.requested.remove(session_id);
        self.fetched.insert(session_id.to_string(), transcript);
    }

    pub fn get(&self, session_id: &str) -> Option<&crate::transcript::Transcript> {
        self.fetched.get(session_id)
    }

    /// Whether this session still needs fetching. Marks it requested, so a
    /// caller polling every frame asks exactly once.
    pub fn should_request(&mut self, session_id: &str) -> bool {
        if self.fetched.contains_key(session_id) || self.requested.contains(session_id) {
            return false;
        }
        self.requested.insert(session_id.to_string());
        true
    }
}

impl Overview {
    /// Open the overview, starting the zoom from wherever it currently is (so
    /// a re-press mid-close reverses rather than restarting).
    pub fn open(&mut self, focus: Option<&str>) {
        // Only a *fully* closed field starts from the attached session.
        // Re-pressing while the zoom is still running back down reverses it
        // and keeps the highlight, so a double flick lands where the user was
        // going rather than being punished with a reset.
        if !self.is_visible() {
            self.focus = focus.map(str::to_string);
        }
        self.open = true;
    }

    /// Begin closing. The zoom runs back down; the field stays drawn until it
    /// reaches zero, which is what makes the commit read as flying into the
    /// session you picked.
    pub fn close(&mut self) {
        self.open = false;
    }

    /// Take the field off screen *now*, with no zoom out.
    ///
    /// The escape hatch for the instant-open gesture: Super opens the field
    /// on the keydown, so a chord like Super+V has to erase it in the same
    /// frame the letter arrives. Zooming out here would leave the cards
    /// washing over the composer for a tenth of a second after the user has
    /// already moved on, which reads as lag rather than as an animation.
    pub fn abort(&mut self) {
        self.open = false;
        self.phase = 0;
    }

    /// Whether the field should be drawn at all.
    pub fn is_visible(&self) -> bool {
        self.open || self.phase > 0
    }

    /// Whether the overview is accepting navigation keys.
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Eased zoom progress, 0.0 to 1.0.
    pub fn phase(&self) -> f64 {
        let linear = f64::from(self.phase) / f64::from(PHASE_MAX);
        // Smoothstep: the field settles rather than slamming to a stop.
        linear * linear * (3.0 - 2.0 * linear)
    }

    pub fn focus(&self) -> Option<&str> {
        self.focus.as_deref()
    }

    pub fn set_focus(&mut self, session_id: &str) {
        self.focus = Some(session_id.to_string());
    }

    /// Advance the zoom by `dt` seconds. Returns true while still animating,
    /// so the event loop knows to schedule another frame.
    pub fn advance(&mut self, dt: f32) -> bool {
        let step = (dt / ZOOM * f32::from(PHASE_MAX)).max(1.0) as u16;
        let before = self.phase;
        self.phase = if self.open {
            self.phase.saturating_add(step).min(PHASE_MAX)
        } else {
            self.phase.saturating_sub(step)
        };
        self.phase != before
    }

    /// Whether the zoom is still running.
    pub fn is_animating(&self) -> bool {
        if self.open {
            self.phase < PHASE_MAX
        } else {
            self.phase > 0
        }
    }

    /// Pin the phase, for captures and tests.
    ///
    /// The state-space nodes need a settled or half-open field without a
    /// clock, exactly as `Caret::pinned` and `Stream::pinned` do, so this is
    /// part of the ordinary API rather than test-only.
    pub fn pinned(open: bool, phase: f64, focus: Option<&str>) -> Self {
        Self {
            open,
            phase: (phase.clamp(0.0, 1.0) * f64::from(PHASE_MAX)) as u16,
            focus: focus.map(str::to_string),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, dir: &str, weight: f64) -> Entry {
        Entry {
            session_id: id.into(),
            working_dir: Some(dir.into()),
            busy: false,
            weight,
        }
    }

    const AREA: (f64, f64, f64, f64) = (0.0, 0.0, 1000.0, 700.0);

    fn field() -> Field {
        layout(
            &[
                entry("a1", "/home/j/jcode", 400.0),
                entry("a2", "/home/j/jcode", 40.0),
                entry("a3", "/home/j/jcode", 4.0),
                entry("b1", "/home/j/site", 200.0),
                entry("b2", "/home/j/site", 10.0),
            ],
            Some("a1"),
            Some("a1"),
            AREA,
        )
    }

    fn by<'a>(field: &'a Field, id: &str) -> &'a Card {
        field
            .cards
            .iter()
            .find(|card| card.session_id == id)
            .unwrap()
    }

    /// The whole premise: a bigger conversation is a wider card.
    #[test]
    fn width_grows_with_the_transcript() {
        let field = field();
        let width = |id: &str| {
            let card = by(&field, id);
            card.rect.2 - card.rect.0
        };
        assert!(
            width("a1") > width("a2"),
            "the heavy session was not the wide one"
        );
        assert!(width("a2") > width("a3"));
    }

    /// The width scale is compressed: a session ten times the size must not
    /// be drawn ten times as wide, or the rest of its row becomes slivers.
    #[test]
    fn sizing_is_compressed_not_linear() {
        let small = width_for(10.0, 1000.0);
        let large = width_for(1000.0, 1000.0);
        assert!(large < small * 10.0, "width scaled linearly with weight");
    }

    /// An empty session still gets a card big enough to see and click.
    #[test]
    fn a_fresh_session_is_still_a_visible_target() {
        let field = layout(&[entry("solo", "/tmp", 0.0)], Some("solo"), None, AREA);
        assert_eq!(field.cards.len(), 1);
        let card = &field.cards[0];
        assert!(card.rect.2 - card.rect.0 >= MIN_CARD_WIDTH * 0.5);
    }

    /// Sessions bucket into one row per directory, in first-appearance order.
    #[test]
    fn rows_are_by_working_dir_and_stable() {
        let field = field();
        assert_eq!(field.rows.len(), 2);
        assert_eq!(field.rows[0].label, "jcode");
        assert_eq!(field.rows[1].label, "site");
        // A row's cards all share a vertical band; the two rows do not.
        let a1 = by(&field, "a1");
        let a2 = by(&field, "a2");
        let b1 = by(&field, "b1");
        assert_eq!(a1.rect.1, a2.rect.1, "one row split vertically");
        assert!(b1.rect.1 > a1.rect.3, "the second row did not stack below");
    }

    /// Cards must not sit on top of one another, or the field is unreadable
    /// and half the sessions are unclickable.
    #[test]
    fn cards_do_not_overlap() {
        let field = field();
        for (i, a) in field.cards.iter().enumerate() {
            for b in &field.cards[i + 1..] {
                let apart = a.rect.2 <= b.rect.0 + 1e-9
                    || b.rect.2 <= a.rect.0 + 1e-9
                    || a.rect.3 <= b.rect.1 + 1e-9
                    || b.rect.3 <= a.rect.1 + 1e-9;
                assert!(apart, "{} and {} overlap", a.session_id, b.session_id);
            }
        }
    }

    /// The field has to fit the window: a card drawn off-page is a session the
    /// user cannot reach.
    #[test]
    fn every_card_fits_inside_the_area() {
        let field = layout(
            &(0..24)
                .map(|n| {
                    entry(
                        &format!("s{n}"),
                        &format!("/home/j/p{}", n % 5),
                        n as f64 * 30.0,
                    )
                })
                .collect::<Vec<_>>(),
            None,
            None,
            AREA,
        );
        for card in &field.cards {
            assert!(card.rect.0 >= AREA.0 - 1.0, "{card:?} left");
            assert!(card.rect.2 <= AREA.2 + 1.0, "{card:?} right");
            assert!(card.rect.1 >= AREA.1 - 1.0, "{card:?} top");
            assert!(card.rect.3 <= AREA.3 + 1.0, "{card:?} bottom");
        }
    }

    /// Layout is a pure function of the input: the field must not drift
    /// between polls of the same session set.
    #[test]
    fn layout_is_deterministic() {
        assert_eq!(field(), field());
    }

    /// Left and right walk the row and stop at its ends: no wrapping around,
    /// which read as the highlight teleporting to the far side.
    #[test]
    fn left_and_right_stop_at_the_rows_ends() {
        let field = field();
        assert_eq!(field.neighbor("a1", Dir::Right).unwrap().session_id, "a2");
        assert_eq!(field.neighbor("a2", Dir::Left).unwrap().session_id, "a1");
        // The edges are walls, not portals.
        assert!(field.neighbor("a3", Dir::Right).is_none());
        assert!(field.neighbor("a1", Dir::Left).is_none());
        // Never into another row: the row is the ring.
        for id in ["a1", "a2"] {
            let right = field.neighbor(id, Dir::Right).unwrap();
            assert_eq!(right.label, "jcode", "{id} wrapped into another row");
        }
    }

    /// Up and down move between rows and stop at the top and bottom.
    #[test]
    fn up_and_down_change_rows() {
        let field = field();
        assert_eq!(field.neighbor("a1", Dir::Down).unwrap().label, "site");
        assert_eq!(field.neighbor("b1", Dir::Up).unwrap().label, "jcode");
        // Clamps at the edges rather than wrapping around.
        assert!(field.neighbor("a1", Dir::Up).is_none());
        assert!(field.neighbor("b1", Dir::Down).is_none());
    }

    /// A dead axis (one row, or a row of one) is distinguishable from a live
    /// axis at its edge, so the caller can fall back only where a key could
    /// never work.
    #[test]
    fn dead_axes_are_reported_but_edges_are_not() {
        let field = field();
        // Two rows and three cards in "jcode": nothing is dead here, even at
        // the edges.
        for dir in [Dir::Left, Dir::Right, Dir::Up, Dir::Down] {
            assert!(!field.axis_is_dead("a1", dir), "{dir:?} reported dead");
        }
        // One row of one card: everything is dead.
        let solo = layout(&[entry("solo", "/tmp", 1.0)], Some("solo"), None, AREA);
        for dir in [Dir::Left, Dir::Right, Dir::Up, Dir::Down] {
            assert!(solo.axis_is_dead("solo", dir), "{dir:?} reported live");
        }
    }

    /// A vertical move lands on the card nearest the one you left, so moving
    /// down and back up returns roughly where you were.
    #[test]
    fn a_vertical_move_keeps_the_column() {
        let field = field();
        let from = by(&field, "a1");
        let landed = field.neighbor("a1", Dir::Down).unwrap();
        // The landing card is the one in the target row with the nearest
        // centre, checked directly against the alternatives.
        for other in field.cards.iter().filter(|card| card.label == "site") {
            assert!(
                (landed.center().0 - from.center().0).abs()
                    <= (other.center().0 - from.center().0).abs() + 1e-9
            );
        }
    }

    /// Moving must never be a no-op that looks like a broken key: from any
    /// card, at least one direction has to go somewhere.
    #[test]
    fn every_card_can_reach_another() {
        let field = field();
        for card in &field.cards {
            let reachable = [Dir::Left, Dir::Right, Dir::Up, Dir::Down]
                .into_iter()
                .filter(|dir| field.neighbor(&card.session_id, *dir).is_some())
                .count();
            assert!(reachable > 0, "{} is stranded", card.session_id);
        }
    }

    /// Tab must visit every session exactly once before wrapping, or some
    /// session is unreachable from the keyboard.
    #[test]
    fn tab_order_covers_every_session_once() {
        let field = field();
        let order = field.reading_order();
        assert_eq!(order.len(), field.cards.len());
        let unique: std::collections::BTreeSet<&str> = order.iter().copied().collect();
        assert_eq!(unique.len(), field.cards.len());
        let mut at = order[0].to_string();
        for _ in 0..field.cards.len() {
            at = field.next_in_order(&at, 1).unwrap().to_string();
        }
        assert_eq!(at, order[0], "the cycle did not wrap to the start");
    }

    /// Clicking inside a card picks it; clicking the paper between cards picks
    /// nothing rather than the nearest.
    #[test]
    fn hit_testing_is_by_the_drawn_rect() {
        let field = field();
        let card = field.cards[0].clone();
        let (cx, cy) = card.center();
        assert_eq!(
            field.hit(cx, cy).map(|hit| &hit.session_id),
            Some(&card.session_id)
        );
        // Just past the right edge is the gap, which belongs to nobody.
        assert_ne!(
            field
                .hit(card.rect.2 + CARD_GAP / 2.0, cy)
                .map(|hit| &hit.session_id),
            Some(&card.session_id),
            "the gap between cards was treated as a hit"
        );
    }

    /// One enormous row must squeeze to fit rather than run off the page.
    #[test]
    fn a_crowded_row_squeezes_to_fit() {
        let field = layout(
            &(0..12)
                .map(|n| entry(&format!("s{n}"), "/home/j/one", 100_000.0))
                .collect::<Vec<_>>(),
            None,
            None,
            AREA,
        );
        for card in &field.cards {
            assert!(card.rect.2 <= AREA.2 + 1.0, "{card:?} ran off the page");
        }
        // Still in one row.
        let tops: std::collections::BTreeSet<i64> = field
            .cards
            .iter()
            .map(|card| card.rect.1.round() as i64)
            .collect();
        assert_eq!(tops.len(), 1, "one directory split into several rows");
    }

    /// The label is the whole of a card's identity, so it must be the name a
    /// human would use and it must differ between sessions.
    #[test]
    fn the_label_is_the_session_name_not_its_hash() {
        assert_eq!(short_id("session_clover_1785130341680_5a8db08"), "clover");
        assert_eq!(
            short_id("session_mushroom_1785129393446_e7007f8"),
            "mushroom"
        );
        // No generated name: still has to produce something, and something
        // that distinguishes one session from the next.
        assert_ne!(short_id("9f0b21d4aa"), short_id("1c93aa4bb0"));
        assert!(!short_id("9f0b21d4aa").is_empty());
    }

    /// Two sessions from one daemon must never draw the same caption, which
    /// is what a hash-derived label would have done across a whole field.
    #[test]
    fn labels_distinguish_sessions_of_a_realistic_field() {
        let ids = [
            "session_clover_1785130341680_5a8db08",
            "session_mushroom_1785129393446_e7007f8",
            "session_pebble_1785130002233_1c93aa4",
        ];
        let labels: std::collections::BTreeSet<String> =
            ids.iter().map(|id| short_id(id)).collect();
        assert_eq!(labels.len(), ids.len());
    }

    /// The zoom must run to completion and stop, in both directions.
    #[test]
    fn the_zoom_opens_and_closes() {
        let mut overview = Overview::default();
        assert!(!overview.is_visible());
        overview.open(Some("a1"));
        assert!(overview.is_open());
        for _ in 0..200 {
            overview.advance(0.016);
        }
        assert!(!overview.is_animating());
        assert!((overview.phase() - 1.0).abs() < 1e-9);
        overview.close();
        assert!(
            overview.is_visible(),
            "the field vanished before the zoom out"
        );
        for _ in 0..200 {
            overview.advance(0.016);
        }
        assert!(!overview.is_visible());
        assert!(!overview.is_animating());
    }

    /// Reopening mid-close keeps the focus the user was on rather than
    /// snapping back, so a double flick is not punished.
    #[test]
    fn reopening_midway_keeps_the_focus() {
        let mut overview = Overview::default();
        overview.open(Some("a1"));
        overview.advance(0.05);
        overview.set_focus("a3");
        overview.close();
        overview.advance(0.02);
        overview.open(Some("a1"));
        assert_eq!(overview.focus(), Some("a3"));
    }
}
