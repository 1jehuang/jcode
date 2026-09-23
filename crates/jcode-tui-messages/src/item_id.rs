//! Stable identity for transcript items.

use std::fmt;

/// Stable identity for one entry in the transcript.
///
/// Minted once, when the item enters the transcript, and preserved across
/// prepends, removals and compaction re-syncs (see the `Transcript` wrapper in
/// `jcode-tui`). Deliberately distinct from `DisplayMessage::stable_cache_hash`:
/// a hash names *content*, an id names a *slot*, so a duplicate or a reordered
/// item cannot masquerade as another. This is what replaces the occurrence
/// ordinal in `ContentPos`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ItemId(pub u64);

impl fmt::Debug for ItemId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.0)
    }
}
