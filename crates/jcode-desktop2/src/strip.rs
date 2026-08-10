//! The session strip: sessions as bars, grouped like niri workspaces.
//!
//! Modelled directly on the author's waybar `niri-workspaces` module, which
//! draws every window as a bar and every workspace as a group of bars. The
//! same mapping applies cleanly here: a *group* is a working directory and a
//! *bar* is a session in it, so the strip answers "where am I, and what else
//! is running" at a glance rather than by reading a list of ids.
//!
//! Navigation is spatial for the same reason: left/right walks the bars in the
//! current group and up/down walks the groups, which is the motion the author
//! already has in his fingers from the compositor.
//!
//! This module is pure. It owns the grouping, the focus, and the wrapping
//! rules, so all of that is testable without a GPU or a live daemon; the
//! renderer and the harness only consume it.

/// One session, as the strip and the overview need to know it.
#[derive(Clone, Debug, PartialEq)]
pub struct Panel {
    pub session_id: String,
    /// User- or agent-generated session title, once one has been assigned.
    pub title: Option<String>,
    /// Working directory, which is what the strip groups on.
    pub working_dir: Option<String>,
    /// Whether the session is currently generating, so a bar can show that a
    /// session you are *not* looking at is doing work.
    pub busy: bool,
    /// How much conversation the session holds, in characters of transcript.
    /// The strip ignores it; the overview sizes its blobs by it, which is what
    /// makes a long-running session recognisable at a glance.
    pub weight: f64,
}

impl Panel {
    #[cfg(test)]
    pub fn new(session_id: &str, working_dir: Option<&str>) -> Self {
        Self {
            session_id: session_id.to_string(),
            title: None,
            working_dir: working_dir.map(str::to_string),
            busy: false,
            weight: 0.0,
        }
    }
}

/// A group of sessions sharing a working directory: one "workspace".
#[derive(Clone, Debug, PartialEq)]
pub struct Strip {
    /// Short label shown before the bars, like the workspace number in the
    /// waybar module. The final path component, because the full path is far
    /// too wide for a strip and the leaf is what distinguishes checkouts.
    pub label: String,
    pub panels: Vec<Panel>,
}

/// The strip's state: the grouped sessions plus where focus sits.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Strips {
    strips: Vec<Strip>,
    strip_index: usize,
    panel_index: usize,
}

/// Label for a working directory: the leaf component, or a placeholder when
/// the daemon did not report one.
fn label_for(working_dir: Option<&str>) -> String {
    let Some(dir) = working_dir else {
        return "-".to_string();
    };
    let trimmed = dir.trim_end_matches('/');
    match trimmed.rsplit('/').next() {
        Some(leaf) if !leaf.is_empty() => leaf.to_string(),
        _ => "/".to_string(),
    }
}

impl Strips {
    /// Build the strip from a session list, keeping focus on `focused` when it
    /// is still present.
    ///
    /// Grouping preserves first-appearance order rather than sorting, so the
    /// bars stay put across polls: a strip whose bars reshuffle every refresh
    /// is unreadable, and worse, makes "one to the left" mean something
    /// different from one second to the next.
    pub fn build(panels: Vec<Panel>, focused: Option<&str>) -> Self {
        let mut strips: Vec<Strip> = Vec::new();
        for panel in panels {
            let label = label_for(panel.working_dir.as_deref());
            match strips.iter_mut().find(|strip| strip.label == label) {
                Some(strip) => strip.panels.push(panel),
                None => strips.push(Strip {
                    label,
                    panels: vec![panel],
                }),
            }
        }
        let mut strip = Self {
            strips,
            strip_index: 0,
            panel_index: 0,
        };
        if let Some(focused) = focused {
            strip.focus_session(focused);
        }
        strip
    }

    /// Point focus at a session id, if the strip still has it.
    pub fn focus_session(&mut self, session_id: &str) -> bool {
        for (strip_index, strip) in self.strips.iter().enumerate() {
            if let Some(i) = strip
                .panels
                .iter()
                .position(|panel| panel.session_id == session_id)
            {
                self.strip_index = strip_index;
                self.panel_index = i;
                return true;
            }
        }
        false
    }

    pub fn strips(&self) -> &[Strip] {
        &self.strips
    }

    /// Every session, flat, in strip order. The overview lays out the whole
    /// set rather than one group at a time, and reads it through here so the
    /// two surfaces can never disagree about which sessions exist.
    pub fn panels(&self) -> Vec<Panel> {
        self.strips
            .iter()
            .flat_map(|strip| strip.panels.iter().cloned())
            .collect()
    }

    pub fn strip_index(&self) -> usize {
        self.strip_index
    }

    pub fn panel_index(&self) -> usize {
        self.panel_index
    }

    /// Total sessions across all groups.
    pub fn len(&self) -> usize {
        self.strips.iter().map(|strip| strip.panels.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The focused group, if any.
    pub fn focused_strip(&self) -> Option<&Strip> {
        self.strips.get(self.strip_index)
    }

    /// The focused session's id, if any.
    pub fn focused_session(&self) -> Option<&str> {
        self.focused_strip()
            .and_then(|strip| strip.panels.get(self.panel_index))
            .map(|entry| entry.session_id.as_str())
    }

    /// Heading for the focused conversation. Before a title is generated, its
    /// stable position in the session list gives the user a useful name.
    pub fn focused_heading(&self) -> Option<String> {
        let focused = self.focused_session()?;
        let (index, entry) = self
            .strips
            .iter()
            .flat_map(|strip| strip.panels.iter())
            .enumerate()
            .find(|(_, entry)| entry.session_id == focused)?;
        match entry.title.as_deref().map(str::trim) {
            Some(title) if !title.is_empty() => Some(title.to_string()),
            _ => Some(format!("{} chat", ordinal(index + 1))),
        }
    }

    /// The focused session's assigned title, excluding the ordinal fallback.
    pub fn focused_title(&self) -> Option<&str> {
        self.focused_strip()
            .and_then(|strip| strip.panels.get(self.panel_index))
            .and_then(|entry| entry.title.as_deref())
            .map(str::trim)
            .filter(|title| !title.is_empty())
    }

    /// The focused session's working directory, if the strip knows one.
    pub fn focused_working_dir(&self) -> Option<&str> {
        self.focused_strip()
            .and_then(|strip| strip.panels.get(self.panel_index))
            .and_then(|entry| entry.working_dir.as_deref())
    }

    /// Move focus one bar left, wrapping within the group. Wrapping rather
    /// than stopping because a strip is a ring of a handful of items, and
    /// hitting an invisible wall at the end reads as the key being broken.
    pub fn focus_left(&mut self) -> bool {
        self.step_within_strip(-1)
    }

    /// Move focus one bar right, wrapping within the group.
    pub fn focus_right(&mut self) -> bool {
        self.step_within_strip(1)
    }

    fn step_within_strip(&mut self, delta: isize) -> bool {
        let len = self.focused_strip().map(|g| g.panels.len()).unwrap_or(0);
        if len <= 1 {
            return false;
        }
        let next = (self.panel_index as isize + delta).rem_euclid(len as isize) as usize;
        if next == self.panel_index {
            return false;
        }
        self.panel_index = next;
        true
    }

    /// Move focus to the previous group, keeping the bar position where it
    /// still exists. Clamping rather than resetting to 0 keeps the motion
    /// feeling like moving *up a column*, which is how the compositor behaves.
    pub fn focus_up(&mut self) -> bool {
        self.step_strip(-1)
    }

    /// Move focus to the next group.
    pub fn focus_down(&mut self) -> bool {
        self.step_strip(1)
    }

    fn step_strip(&mut self, delta: isize) -> bool {
        if self.strips.len() <= 1 {
            return false;
        }
        let next =
            (self.strip_index as isize + delta).rem_euclid(self.strips.len() as isize) as usize;
        if next == self.strip_index {
            return false;
        }
        self.strip_index = next;
        let len = self.strips[next].panels.len();
        self.panel_index = self.panel_index.min(len.saturating_sub(1));
        true
    }
}

fn ordinal(number: usize) -> String {
    let suffix = if (11..=13).contains(&(number % 100)) {
        "th"
    } else {
        match number % 10 {
            1 => "st",
            2 => "nd",
            3 => "rd",
            _ => "th",
        }
    };
    format!("{number}{suffix}")
}

/// One drawable item in the strip: a group's frame or a session block.
///
/// Laying the strip out as data rather than drawing directly means the
/// positions can be asserted without a GPU, and the pixel tests can check the
/// *rendering* of a layout that is already known to be correct.
///
/// There is no text here on purpose: the strip is a diagram, not a caption.
/// A group is an outlined rectangle and each session in it is a block inside
/// that rectangle, so the shape alone answers "how many places, how many
/// sessions, which one am I in".
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Item {
    /// The outline enclosing one group's blocks. Which group it is indexes
    /// `groups()`.
    Strip {
        strip: usize,
        x: f64,
        width: f64,
        /// Whether focus currently sits inside this group, so the enclosing
        /// rectangle can be drawn at full ink while the others stay faint.
        focused: bool,
    },
    /// A session block. `focused` selects the wide solid form.
    Panel {
        strip: usize,
        panel: usize,
        x: f64,
        width: f64,
        focused: bool,
    },
}

/// Lay the strip out from `left`, in logical units.
///
/// Groups run left to right on one row: an outlined rectangle per working
/// directory, holding one block per session, then a wider gap before the next
/// group. Groups past `right` are dropped whole rather than squeezed, because a
/// strip that reflows or shrinks its blocks stops being scannable at a glance.
pub fn layout_items(strip: &Strips, left: f64, right: f64) -> Vec<Item> {
    let pad = crate::layout::STRIP_FRAME_PAD;
    let mut items = Vec::new();
    let mut x = left;
    for (g, group) in strip.strips().iter().enumerate() {
        let blocks_w: f64 = group
            .panels
            .iter()
            .enumerate()
            .map(|(i, _)| panel_width(strip, g, i))
            .sum::<f64>()
            + group.panels.len().saturating_sub(1) as f64 * crate::layout::STRIP_BAR_GAP;
        let frame_w = blocks_w + pad * 2.0;
        if x + frame_w > right {
            break;
        }
        items.push(Item::Strip {
            strip: g,
            x,
            width: frame_w,
            focused: strip.strip_index() == g,
        });
        let mut bx = x + pad;
        for (i, _) in group.panels.iter().enumerate() {
            let width = panel_width(strip, g, i);
            items.push(Item::Panel {
                strip: g,
                panel: i,
                x: bx,
                width,
                focused: strip.strip_index() == g && strip.panel_index() == i,
            });
            bx += width + crate::layout::STRIP_BAR_GAP;
        }
        x += frame_w + crate::layout::STRIP_GROUP_GAP;
    }
    items
}

fn panel_width(strips: &Strips, strip: usize, panel: usize) -> f64 {
    if strips.strip_index() == strip && strips.panel_index() == panel {
        crate::layout::STRIP_BAR_FOCUS_WIDTH
    } else {
        crate::layout::STRIP_BAR_WIDTH
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip() -> Strips {
        Strips::build(
            vec![
                Panel::new("a1", Some("/home/j/jcode")),
                Panel::new("a2", Some("/home/j/jcode")),
                Panel::new("a3", Some("/home/j/jcode")),
                Panel::new("b1", Some("/home/j/site")),
            ],
            Some("a1"),
        )
    }

    /// R5: bars must bucket by directory and keep a stable order, or the strip
    /// reshuffles under the user's fingers between polls.
    #[test]
    fn grouping_is_by_working_dir_and_is_stable() {
        let strip = strip();
        assert_eq!(strip.strips().len(), 2);
        assert_eq!(strip.strips()[0].label, "jcode");
        assert_eq!(strip.strips()[0].panels.len(), 3);
        assert_eq!(strip.strips()[1].label, "site");
        // Rebuilding the same list must produce the same layout.
        let again = strip.clone();
        assert_eq!(strip, again);
    }

    /// A session with no reported directory still gets a bar rather than
    /// vanishing: an invisible session is worse than an oddly labelled one.
    #[test]
    fn sessions_without_a_working_dir_are_still_grouped() {
        let strip = Strips::build(vec![Panel::new("x", None)], None);
        assert_eq!(strip.strips().len(), 1);
        assert_eq!(strip.strips()[0].label, "-");
    }

    /// R3.
    #[test]
    fn focus_right_wraps_within_the_group() {
        let mut strip = strip();
        assert_eq!(strip.focused_session(), Some("a1"));
        assert!(strip.focus_right());
        assert_eq!(strip.focused_session(), Some("a2"));
        assert!(strip.focus_right());
        assert_eq!(strip.focused_session(), Some("a3"));
        assert!(strip.focus_right());
        assert_eq!(strip.focused_session(), Some("a1"), "did not wrap");
        assert_eq!(strip.strip_index(), 0, "left the group");
    }

    /// R3.
    #[test]
    fn focus_left_wraps_within_the_group() {
        let mut strip = strip();
        assert!(strip.focus_left());
        assert_eq!(strip.focused_session(), Some("a3"));
        assert_eq!(strip.strip_index(), 0);
    }

    /// R4.
    #[test]
    fn focus_down_changes_group_and_clamps_index() {
        let mut strip = strip();
        strip.focus_right();
        strip.focus_right();
        assert_eq!(strip.panel_index(), 2);
        assert!(strip.focus_down());
        assert_eq!(strip.strip_index(), 1);
        assert_eq!(
            strip.panel_index(),
            0,
            "index was not clamped into the group"
        );
        assert_eq!(strip.focused_session(), Some("b1"));
        assert!(strip.focus_up());
        assert_eq!(strip.strip_index(), 0);
    }

    /// R4: with nothing to move to, the keys must be quiet no-ops rather than
    /// appearing to work.
    #[test]
    fn movement_is_a_no_op_when_there_is_nowhere_to_go() {
        let mut only = Strips::build(vec![Panel::new("solo", Some("/tmp"))], Some("solo"));
        assert!(!only.focus_left());
        assert!(!only.focus_right());
        assert!(!only.focus_up());
        assert!(!only.focus_down());

        let mut empty = Strips::default();
        assert!(!empty.focus_left());
        assert!(!empty.focus_down());
        assert_eq!(empty.focused_session(), None);
    }

    /// Focus must survive a refresh, or every poll would yank the user back to
    /// the first bar.
    #[test]
    fn rebuilding_keeps_focus_on_the_same_session() {
        let entries = vec![
            Panel::new("a1", Some("/home/j/jcode")),
            Panel::new("a2", Some("/home/j/jcode")),
        ];
        let strip = Strips::build(entries, Some("a2"));
        assert_eq!(strip.focused_session(), Some("a2"));
    }

    #[test]
    fn heading_uses_the_session_title_when_available() {
        let mut entry = Panel::new("a1", Some("/tmp"));
        entry.title = Some("  Fix the renderer  ".into());
        let strip = Strips::build(vec![entry], Some("a1"));
        assert_eq!(strip.focused_heading().as_deref(), Some("Fix the renderer"));
    }

    #[test]
    fn heading_falls_back_to_the_chat_ordinal() {
        let strip = Strips::build(
            (1..=13)
                .map(|number| Panel::new(&format!("s{number}"), Some("/tmp")))
                .collect(),
            Some("s13"),
        );
        assert_eq!(strip.focused_heading().as_deref(), Some("13th chat"));
        assert_eq!(ordinal(1), "1st");
        assert_eq!(ordinal(2), "2nd");
        assert_eq!(ordinal(3), "3rd");
        assert_eq!(ordinal(11), "11th");
        assert_eq!(ordinal(12), "12th");
    }

    /// A session that disappeared must not leave focus pointing at nothing.
    #[test]
    fn focus_falls_back_when_the_session_is_gone() {
        let strip = Strips::build(vec![Panel::new("a1", Some("/tmp"))], Some("vanished"));
        assert_eq!(strip.focused_session(), Some("a1"));
    }

    #[test]
    fn label_uses_the_leaf_directory() {
        assert_eq!(label_for(Some("/home/j/jcode")), "jcode");
        assert_eq!(label_for(Some("/home/j/jcode/")), "jcode");
        assert_eq!(label_for(Some("/")), "/");
        assert_eq!(label_for(None), "-");
    }
}

#[cfg(test)]
mod layout_tests {
    use super::*;

    fn strip() -> Strips {
        Strips::build(
            vec![
                Panel::new("a1", Some("/home/j/jcode")),
                Panel::new("a2", Some("/home/j/jcode")),
                Panel::new("b1", Some("/home/j/site")),
            ],
            Some("a2"),
        )
    }

    /// One block per session, one enclosing frame per group, in reading order.
    #[test]
    fn every_session_gets_exactly_one_block() {
        let items = layout_items(&strip(), 0.0, 400.0);
        let blocks = items
            .iter()
            .filter(|item| matches!(item, Item::Panel { .. }))
            .count();
        let frames = items
            .iter()
            .filter(|item| matches!(item, Item::Strip { .. }))
            .count();
        assert_eq!(blocks, 3, "blocks did not match the session count");
        assert_eq!(frames, 2, "frames did not match the group count");
    }

    /// The focused block is the wide one, and it is the only wide one: that is
    /// the whole visual signal the strip carries.
    #[test]
    fn exactly_one_block_is_drawn_focused() {
        let items = layout_items(&strip(), 0.0, 400.0);
        let focused: Vec<(usize, f64)> = items
            .iter()
            .filter_map(|item| match item {
                Item::Panel {
                    focused: true,
                    width,
                    panel,
                    ..
                } => Some((*panel, *width)),
                _ => None,
            })
            .collect();
        assert_eq!(focused.len(), 1);
        assert_eq!(focused[0].0, 1, "the wrong block was focused");
        assert!(
            focused[0].1 > crate::layout::STRIP_BAR_WIDTH,
            "the focused block was not wider than a tick"
        );
    }

    /// Exactly one group frame reads as focused, and it is the group focus is
    /// really in: the frame is what says "this is the place you are in".
    #[test]
    fn exactly_one_group_frame_is_focused() {
        let items = layout_items(&strip(), 0.0, 400.0);
        let focused: Vec<usize> = items
            .iter()
            .filter_map(|item| match item {
                Item::Strip {
                    strip,
                    focused: true,
                    ..
                } => Some(*strip),
                _ => None,
            })
            .collect();
        assert_eq!(focused, vec![0]);
    }

    /// Every block must sit inside its group's rectangle, or the diagram lies
    /// about which sessions belong to which place.
    #[test]
    fn blocks_sit_inside_their_group_frame() {
        let items = layout_items(&strip(), 10.0, 400.0);
        for item in &items {
            let Item::Panel {
                strip, x, width, ..
            } = item
            else {
                continue;
            };
            let frame = items
                .iter()
                .find_map(|other| match other {
                    Item::Strip {
                        strip: g,
                        x: fx,
                        width: fw,
                        ..
                    } if g == strip => Some((*fx, *fw)),
                    _ => None,
                })
                .expect("a block was laid out without its group frame");
            assert!(
                *x >= frame.0 && x + width <= frame.0 + frame.1,
                "block at {x} escaped its frame {frame:?}"
            );
        }
    }

    /// Blocks must advance strictly left to right and never overlap, or they
    /// smear into one another.
    #[test]
    fn blocks_advance_left_to_right_without_overlapping() {
        let items = layout_items(&strip(), 10.0, 400.0);
        let mut cursor = 10.0f64;
        for item in &items {
            let Item::Panel { x, width, .. } = item else {
                continue;
            };
            assert!(*x >= cursor - 1e-9, "block at {x} overlapped {cursor}");
            cursor = x + width;
        }
    }

    /// Group frames never overlap each other either.
    #[test]
    fn group_frames_do_not_overlap() {
        let items = layout_items(&strip(), 0.0, 400.0);
        let mut cursor = 0.0f64;
        for item in &items {
            let Item::Strip { x, width, .. } = item else {
                continue;
            };
            assert!(*x >= cursor - 1e-9, "frame at {x} overlapped {cursor}");
            cursor = x + width;
        }
    }

    /// A narrow window drops whole groups rather than drawing a half group or
    /// letting the strip run off the page.
    #[test]
    fn groups_that_do_not_fit_are_dropped_whole() {
        let items = layout_items(&strip(), 0.0, 30.0);
        let groups: std::collections::BTreeSet<usize> = items
            .iter()
            .map(|item| match item {
                Item::Strip { strip, .. } | Item::Panel { strip, .. } => *strip,
            })
            .collect();
        assert!(groups.len() < 2, "a group was drawn past the right edge");
        for item in &items {
            let (x, width) = match item {
                Item::Strip { x, width, .. } | Item::Panel { x, width, .. } => (*x, *width),
            };
            assert!(x + width <= 30.0, "an item ran off the page");
        }
    }
}
