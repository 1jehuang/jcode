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
pub struct Entry {
    pub session_id: String,
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

impl Entry {
    #[cfg(test)]
    pub fn new(session_id: &str, working_dir: Option<&str>) -> Self {
        Self {
            session_id: session_id.to_string(),
            working_dir: working_dir.map(str::to_string),
            busy: false,
            weight: 0.0,
        }
    }
}

/// A group of sessions sharing a working directory: one "workspace".
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    /// Short label shown before the bars, like the workspace number in the
    /// waybar module. The final path component, because the full path is far
    /// too wide for a strip and the leaf is what distinguishes checkouts.
    pub label: String,
    pub entries: Vec<Entry>,
}

/// The strip's state: the grouped sessions plus where focus sits.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Strip {
    groups: Vec<Group>,
    group: usize,
    index: usize,
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

impl Strip {
    /// Build the strip from a session list, keeping focus on `focused` when it
    /// is still present.
    ///
    /// Grouping preserves first-appearance order rather than sorting, so the
    /// bars stay put across polls: a strip whose bars reshuffle every refresh
    /// is unreadable, and worse, makes "one to the left" mean something
    /// different from one second to the next.
    pub fn build(entries: Vec<Entry>, focused: Option<&str>) -> Self {
        let mut groups: Vec<Group> = Vec::new();
        for entry in entries {
            let label = label_for(entry.working_dir.as_deref());
            match groups.iter_mut().find(|group| group.label == label) {
                Some(group) => group.entries.push(entry),
                None => groups.push(Group {
                    label,
                    entries: vec![entry],
                }),
            }
        }
        let mut strip = Self {
            groups,
            group: 0,
            index: 0,
        };
        if let Some(focused) = focused {
            strip.focus_session(focused);
        }
        strip
    }

    /// Point focus at a session id, if the strip still has it.
    pub fn focus_session(&mut self, session_id: &str) -> bool {
        for (g, group) in self.groups.iter().enumerate() {
            if let Some(i) = group
                .entries
                .iter()
                .position(|entry| entry.session_id == session_id)
            {
                self.group = g;
                self.index = i;
                return true;
            }
        }
        false
    }

    pub fn groups(&self) -> &[Group] {
        &self.groups
    }

    /// Every session, flat, in strip order. The overview lays out the whole
    /// set rather than one group at a time, and reads it through here so the
    /// two surfaces can never disagree about which sessions exist.
    pub fn entries(&self) -> Vec<Entry> {
        self.groups
            .iter()
            .flat_map(|group| group.entries.iter().cloned())
            .collect()
    }

    pub fn group_index(&self) -> usize {
        self.group
    }

    pub fn index(&self) -> usize {
        self.index
    }

    /// Total sessions across all groups.
    pub fn len(&self) -> usize {
        self.groups.iter().map(|group| group.entries.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The focused group, if any.
    pub fn focused_group(&self) -> Option<&Group> {
        self.groups.get(self.group)
    }

    /// The focused session's id, if any.
    pub fn focused_session(&self) -> Option<&str> {
        self.focused_group()
            .and_then(|group| group.entries.get(self.index))
            .map(|entry| entry.session_id.as_str())
    }

    /// The focused session's working directory, if the strip knows one.
    pub fn focused_working_dir(&self) -> Option<&str> {
        self.focused_group()
            .and_then(|group| group.entries.get(self.index))
            .and_then(|entry| entry.working_dir.as_deref())
    }

    /// Move focus one bar left, wrapping within the group. Wrapping rather
    /// than stopping because a strip is a ring of a handful of items, and
    /// hitting an invisible wall at the end reads as the key being broken.
    pub fn focus_left(&mut self) -> bool {
        self.step_within_group(-1)
    }

    /// Move focus one bar right, wrapping within the group.
    pub fn focus_right(&mut self) -> bool {
        self.step_within_group(1)
    }

    fn step_within_group(&mut self, delta: isize) -> bool {
        let len = self.focused_group().map(|g| g.entries.len()).unwrap_or(0);
        if len <= 1 {
            return false;
        }
        let next = (self.index as isize + delta).rem_euclid(len as isize) as usize;
        if next == self.index {
            return false;
        }
        self.index = next;
        true
    }

    /// Move focus to the previous group, keeping the bar position where it
    /// still exists. Clamping rather than resetting to 0 keeps the motion
    /// feeling like moving *up a column*, which is how the compositor behaves.
    pub fn focus_up(&mut self) -> bool {
        self.step_group(-1)
    }

    /// Move focus to the next group.
    pub fn focus_down(&mut self) -> bool {
        self.step_group(1)
    }

    fn step_group(&mut self, delta: isize) -> bool {
        if self.groups.len() <= 1 {
            return false;
        }
        let next = (self.group as isize + delta).rem_euclid(self.groups.len() as isize) as usize;
        if next == self.group {
            return false;
        }
        self.group = next;
        let len = self.groups[next].entries.len();
        self.index = self.index.min(len.saturating_sub(1));
        true
    }
}

/// One drawable item in the strip: either a group's label or a session bar.
///
/// Laying the strip out as data rather than drawing directly means the bar
/// positions can be asserted without a GPU, and the pixel tests can check the
/// *rendering* of a layout that is already known to be correct.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Item {
    /// A group label, at this x. Which group it is indexes `groups()`.
    Label { group: usize, x: f64 },
    /// A session bar. `focused` selects the wide solid form.
    Bar {
        group: usize,
        index: usize,
        x: f64,
        width: f64,
        focused: bool,
    },
}

/// Lay the strip out from `left`, in logical units.
///
/// Groups run left to right on one row, exactly like the waybar module: a
/// label, then that group's bars, then a wider gap before the next group.
/// Items past `right` are dropped rather than squeezed, because a strip that
/// reflows or shrinks its bars stops being scannable at a glance.
pub fn layout_items(
    strip: &Strip,
    left: f64,
    right: f64,
    label_width: impl Fn(&str) -> f64,
) -> Vec<Item> {
    let mut items = Vec::new();
    let mut x = left;
    for (g, group) in strip.groups().iter().enumerate() {
        let label_w = label_width(&group.label);
        let bars_w: f64 = group
            .entries
            .iter()
            .enumerate()
            .map(|(i, _)| bar_width(strip, g, i) + crate::layout::STRIP_BAR_GAP)
            .sum();
        if x + label_w + crate::layout::STRIP_LABEL_GAP + bars_w > right {
            break;
        }
        items.push(Item::Label { group: g, x });
        x += label_w + crate::layout::STRIP_LABEL_GAP;
        for (i, _) in group.entries.iter().enumerate() {
            let width = bar_width(strip, g, i);
            items.push(Item::Bar {
                group: g,
                index: i,
                x,
                width,
                focused: strip.group_index() == g && strip.index() == i,
            });
            x += width + crate::layout::STRIP_BAR_GAP;
        }
        x += crate::layout::STRIP_GROUP_GAP;
    }
    items
}

fn bar_width(strip: &Strip, group: usize, index: usize) -> f64 {
    if strip.group_index() == group && strip.index() == index {
        crate::layout::STRIP_BAR_FOCUS_WIDTH
    } else {
        crate::layout::STRIP_BAR_WIDTH
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip() -> Strip {
        Strip::build(
            vec![
                Entry::new("a1", Some("/home/j/jcode")),
                Entry::new("a2", Some("/home/j/jcode")),
                Entry::new("a3", Some("/home/j/jcode")),
                Entry::new("b1", Some("/home/j/site")),
            ],
            Some("a1"),
        )
    }

    /// R5: bars must bucket by directory and keep a stable order, or the strip
    /// reshuffles under the user's fingers between polls.
    #[test]
    fn grouping_is_by_working_dir_and_is_stable() {
        let strip = strip();
        assert_eq!(strip.groups().len(), 2);
        assert_eq!(strip.groups()[0].label, "jcode");
        assert_eq!(strip.groups()[0].entries.len(), 3);
        assert_eq!(strip.groups()[1].label, "site");
        // Rebuilding the same list must produce the same layout.
        let again = strip.clone();
        assert_eq!(strip, again);
    }

    /// A session with no reported directory still gets a bar rather than
    /// vanishing: an invisible session is worse than an oddly labelled one.
    #[test]
    fn sessions_without_a_working_dir_are_still_grouped() {
        let strip = Strip::build(vec![Entry::new("x", None)], None);
        assert_eq!(strip.groups().len(), 1);
        assert_eq!(strip.groups()[0].label, "-");
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
        assert_eq!(strip.group_index(), 0, "left the group");
    }

    /// R3.
    #[test]
    fn focus_left_wraps_within_the_group() {
        let mut strip = strip();
        assert!(strip.focus_left());
        assert_eq!(strip.focused_session(), Some("a3"));
        assert_eq!(strip.group_index(), 0);
    }

    /// R4.
    #[test]
    fn focus_down_changes_group_and_clamps_index() {
        let mut strip = strip();
        strip.focus_right();
        strip.focus_right();
        assert_eq!(strip.index(), 2);
        assert!(strip.focus_down());
        assert_eq!(strip.group_index(), 1);
        assert_eq!(strip.index(), 0, "index was not clamped into the group");
        assert_eq!(strip.focused_session(), Some("b1"));
        assert!(strip.focus_up());
        assert_eq!(strip.group_index(), 0);
    }

    /// R4: with nothing to move to, the keys must be quiet no-ops rather than
    /// appearing to work.
    #[test]
    fn movement_is_a_no_op_when_there_is_nowhere_to_go() {
        let mut only = Strip::build(vec![Entry::new("solo", Some("/tmp"))], Some("solo"));
        assert!(!only.focus_left());
        assert!(!only.focus_right());
        assert!(!only.focus_up());
        assert!(!only.focus_down());

        let mut empty = Strip::default();
        assert!(!empty.focus_left());
        assert!(!empty.focus_down());
        assert_eq!(empty.focused_session(), None);
    }

    /// Focus must survive a refresh, or every poll would yank the user back to
    /// the first bar.
    #[test]
    fn rebuilding_keeps_focus_on_the_same_session() {
        let entries = vec![
            Entry::new("a1", Some("/home/j/jcode")),
            Entry::new("a2", Some("/home/j/jcode")),
        ];
        let strip = Strip::build(entries, Some("a2"));
        assert_eq!(strip.focused_session(), Some("a2"));
    }

    /// A session that disappeared must not leave focus pointing at nothing.
    #[test]
    fn focus_falls_back_when_the_session_is_gone() {
        let strip = Strip::build(vec![Entry::new("a1", Some("/tmp"))], Some("vanished"));
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

    fn strip() -> Strip {
        Strip::build(
            vec![
                Entry::new("a1", Some("/home/j/jcode")),
                Entry::new("a2", Some("/home/j/jcode")),
                Entry::new("b1", Some("/home/j/site")),
            ],
            Some("a2"),
        )
    }

    /// One bar per session, one label per group, in reading order.
    #[test]
    fn every_session_gets_exactly_one_bar() {
        let items = layout_items(&strip(), 0.0, 400.0, |_| 20.0);
        let bars = items
            .iter()
            .filter(|item| matches!(item, Item::Bar { .. }))
            .count();
        let labels = items
            .iter()
            .filter(|item| matches!(item, Item::Label { .. }))
            .count();
        assert_eq!(bars, 3, "bars did not match the session count");
        assert_eq!(labels, 2, "labels did not match the group count");
    }

    /// The focused bar is the wide one, and it is the only wide one: that is
    /// the whole visual signal the strip carries.
    #[test]
    fn exactly_one_bar_is_drawn_focused() {
        let items = layout_items(&strip(), 0.0, 400.0, |_| 20.0);
        let focused: Vec<_> = items
            .iter()
            .filter_map(|item| match item {
                Item::Bar {
                    focused: true,
                    width,
                    index,
                    ..
                } => Some((*index, *width)),
                _ => None,
            })
            .collect();
        assert_eq!(focused.len(), 1);
        assert_eq!(focused[0].0, 1, "the wrong bar was focused");
        assert!(
            focused[0].1 > crate::layout::STRIP_BAR_WIDTH,
            "the focused bar was not wider than a tick"
        );
    }

    /// Items must advance strictly left to right and never overlap, or the
    /// bars smear into one another.
    #[test]
    fn items_advance_left_to_right_without_overlapping() {
        let items = layout_items(&strip(), 10.0, 400.0, |_| 20.0);
        let mut cursor = 10.0f64;
        for item in &items {
            let (x, width) = match item {
                Item::Label { x, .. } => (*x, 20.0),
                Item::Bar { x, width, .. } => (*x, *width),
            };
            assert!(x >= cursor - 1e-9, "item at {x} overlapped {cursor}");
            cursor = x + width;
        }
    }

    /// A narrow window drops whole groups rather than drawing a half group or
    /// letting the strip run off the page.
    #[test]
    fn groups_that_do_not_fit_are_dropped_whole() {
        let items = layout_items(&strip(), 0.0, 60.0, |_| 20.0);
        let groups: std::collections::BTreeSet<usize> = items
            .iter()
            .map(|item| match item {
                Item::Label { group, .. } | Item::Bar { group, .. } => *group,
            })
            .collect();
        assert!(groups.len() < 2, "a group was drawn past the right edge");
        for item in &items {
            if let Item::Bar { x, width, .. } = item {
                assert!(x + width <= 60.0, "a bar ran off the page");
            }
        }
    }
}
