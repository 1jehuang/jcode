//! The transcript container: the `App` transcript plus a parallel item-id
//! vector, kept in lockstep by the only methods that can mutate it.

use jcode_tui_messages::{DisplayMessage, ItemId};
use std::ops::Deref;

/// The transcript: `Vec<DisplayMessage>` plus one stable [`ItemId`] per item.
///
/// The two vectors always have the same length because they are only ever
/// touched together, by these methods. Reads go through
/// `Deref<Target = [DisplayMessage]>`, so every existing reader keeps compiling
/// unchanged. There is deliberately **no `DerefMut`**: a caller holding
/// `&mut Transcript` still cannot push past the wrapper or reorder one vector
/// without the other, so a desync is a compile error rather than a bug.
#[derive(Clone, Debug, Default)]
pub struct Transcript {
    items: Vec<DisplayMessage>,
    ids: Vec<ItemId>,
    next_id: u64,
}

impl Transcript {
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a transcript from a freshly loaded list. Every item gets a new id,
    /// which is correct: there is no previous identity to preserve.
    pub fn from_items(items: Vec<DisplayMessage>) -> Self {
        let mut transcript = Self::new();
        transcript.replace(items);
        transcript
    }

    pub fn ids(&self) -> impl Iterator<Item = ItemId> + '_ {
        self.ids.iter().copied()
    }

    pub fn id_at(&self, idx: usize) -> Option<ItemId> {
        self.ids.get(idx).copied()
    }

    /// Mutable access to one item. Cannot change the length, so it cannot
    /// desync the id vector; this is the safe escape hatch for in-place edits
    /// and streaming updates.
    pub fn get_mut(&mut self, idx: usize) -> Option<&mut DisplayMessage> {
        self.items.get_mut(idx)
    }

    pub fn last_mut(&mut self) -> Option<&mut DisplayMessage> {
        self.items.last_mut()
    }

    /// In-place mutation of every item. Yields `&mut DisplayMessage`, which
    /// cannot insert or remove, so the id vector stays valid.
    pub fn iter_mut(&mut self) -> impl DoubleEndedIterator<Item = &mut DisplayMessage> {
        self.items.iter_mut()
    }

    fn mint(&mut self) -> ItemId {
        let id = ItemId(self.next_id);
        self.next_id += 1;
        id
    }

    pub fn push(&mut self, msg: DisplayMessage) -> ItemId {
        let id = self.mint();
        self.items.push(msg);
        self.ids.push(id);
        id
    }

    pub fn pop(&mut self) -> Option<DisplayMessage> {
        self.ids.pop();
        self.items.pop()
    }

    pub fn remove(&mut self, idx: usize) -> DisplayMessage {
        self.ids.remove(idx);
        self.items.remove(idx)
    }

    pub fn insert(&mut self, idx: usize, msg: DisplayMessage) -> ItemId {
        let id = self.mint();
        self.items.insert(idx, msg);
        self.ids.insert(idx, id);
        id
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.ids.clear();
    }

    pub fn retain(&mut self, mut keep: impl FnMut(&DisplayMessage) -> bool) {
        let mut i = 0;
        while i < self.items.len() {
            if keep(&self.items[i]) {
                i += 1;
            } else {
                self.items.remove(i);
                self.ids.remove(i);
            }
        }
    }

    /// Replace the whole transcript, preserving ids positionally where content
    /// is unchanged.
    ///
    /// Keeps the matching prefix and the matching suffix, so a prepend or an
    /// append leaves every surviving item's id alone, and mints fresh ids only
    /// for the changed middle (an edit, or a reorder). Content is compared by
    /// [`DisplayMessage::stable_cache_hash`], the same notion the frame's reuse
    /// matchers use, which keeps the load-bearing invariant: **equal id implies
    /// equal content**, so the matchers can trust an id match.
    ///
    /// An unrecognizable change lands in the middle and fails safe: the anchor
    /// stops resolving and the reader stays put rather than jumping.
    pub fn replace(&mut self, items: Vec<DisplayMessage>) {
        let old = &self.items;
        let (n, m) = (items.len(), old.len());
        let limit = n.min(m);
        let hash = |msg: &DisplayMessage| msg.stable_cache_hash();

        let mut prefix = 0;
        while prefix < limit && hash(&old[prefix]) == hash(&items[prefix]) {
            prefix += 1;
        }
        let mut suffix = 0;
        while suffix < limit - prefix && hash(&old[m - 1 - suffix]) == hash(&items[n - 1 - suffix])
        {
            suffix += 1;
        }

        let mut ids = Vec::with_capacity(n);
        for i in 0..n {
            if i < prefix {
                ids.push(self.ids[i]);
            } else if i >= n - suffix {
                ids.push(self.ids[m - suffix + (i - (n - suffix))]);
            } else {
                ids.push(self.mint());
            }
        }

        self.items = items;
        self.ids = ids;
    }
}

impl Deref for Transcript {
    type Target = [DisplayMessage];

    fn deref(&self) -> &[DisplayMessage] {
        &self.items
    }
}

impl<'a> IntoIterator for &'a Transcript {
    type Item = &'a DisplayMessage;
    type IntoIter = std::slice::Iter<'a, DisplayMessage>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn users(contents: &[&str]) -> Vec<DisplayMessage> {
        contents.iter().map(|c| DisplayMessage::user(*c)).collect()
    }

    #[test]
    fn push_pop_remove_keep_the_vectors_in_step() {
        let mut t = Transcript::new();
        let a = t.push(DisplayMessage::user("a"));
        let b = t.push(DisplayMessage::user("b"));
        let c = t.push(DisplayMessage::user("c"));

        assert_eq!(t.len(), 3);
        assert_eq!(t.ids().collect::<Vec<_>>(), vec![a, b, c]);

        assert_eq!(t.remove(1).content, "b");
        assert_eq!(t.ids().collect::<Vec<_>>(), vec![a, c]);

        assert_eq!(t.pop().unwrap().content, "c");
        assert_eq!(t.ids().collect::<Vec<_>>(), vec![a]);
        assert_eq!(t.ids().count(), t.len());
    }

    #[test]
    fn retain_removes_ids_too() {
        let mut t = Transcript::from_items(users(&["a", "b", "c", "b"]));
        let before: Vec<_> = t.ids().collect();
        t.retain(|m| m.content != "b");
        assert_eq!(
            t.iter().map(|m| m.content.as_str()).collect::<Vec<_>>(),
            vec!["a", "c"]
        );
        assert_eq!(t.ids().collect::<Vec<_>>(), vec![before[0], before[2]]);
    }

    #[test]
    fn append_preserves_existing_ids() {
        let mut t = Transcript::from_items(users(&["a", "b"]));
        let (a, b) = (t.id_at(0).unwrap(), t.id_at(1).unwrap());
        t.replace(users(&["a", "b", "c"]));
        assert_eq!(t.id_at(0), Some(a));
        assert_eq!(t.id_at(1), Some(b));
        assert_ne!(t.id_at(2), Some(a));
    }

    #[test]
    fn prepend_preserves_the_tail_ids() {
        let mut t = Transcript::from_items(users(&["b", "c"]));
        let (b, c) = (t.id_at(0).unwrap(), t.id_at(1).unwrap());
        // Compacted history loads above the existing pair.
        t.replace(users(&["a", "b", "c"]));
        assert_eq!(t.id_at(1), Some(b));
        assert_eq!(t.id_at(2), Some(c));
        assert_ne!(t.id_at(0), Some(b)); // the new head is fresh
    }

    #[test]
    fn duplicate_prepend_keeps_ids_for_hash_equal_content() {
        // Identical content is genuinely ambiguous: a prepend of an identical
        // message cannot be told apart from an append by content alone. What
        // must hold is the invariant the reuse matchers rely on -- every reused
        // id still maps to hash-equal content, the two old ids survive exactly
        // once, and exactly one fresh id is minted.
        let mut t = Transcript::from_items(users(&["a", "a"]));
        let old: Vec<_> = t.ids().collect();
        t.replace(users(&["a", "a", "a"]));
        let new: Vec<_> = t.ids().collect();
        assert_eq!(new.len(), 3);
        assert_eq!(t.ids().count(), t.len());
        for id in &old {
            assert_eq!(new.iter().filter(|n| *n == id).count(), 1);
        }
        assert_eq!(new.iter().filter(|n| !old.contains(n)).count(), 1);
    }

    #[test]
    fn edit_in_the_middle_mints_only_the_middle() {
        let mut t = Transcript::from_items(users(&["a", "b", "c"]));
        let (a, c) = (t.id_at(0).unwrap(), t.id_at(2).unwrap());
        let b = t.id_at(1).unwrap();
        t.replace(users(&["a", "B", "c"]));
        assert_eq!(t.id_at(0), Some(a));
        assert_eq!(t.id_at(2), Some(c));
        assert_ne!(t.id_at(1), Some(b));
    }

    #[test]
    fn reorder_fails_safe_by_minting_all_fresh() {
        let mut t = Transcript::from_items(users(&["a", "b"]));
        let (a, b) = (t.id_at(0).unwrap(), t.id_at(1).unwrap());
        t.replace(users(&["b", "a"]));
        assert_ne!(t.id_at(0), Some(a));
        assert_ne!(t.id_at(1), Some(b));
        assert_ne!(t.id_at(0), t.id_at(1));
    }

    #[test]
    fn clear_empties_both_vectors() {
        let mut t = Transcript::from_items(users(&["a", "b"]));
        t.clear();
        assert!(t.is_empty());
        assert_eq!(t.ids().count(), 0);
    }
}
