//! Ghost hints for the empty composer.
//!
//! An empty field with no hint reads as inert, and a fixed "message jcode"
//! label is a caption you stop seeing after a day. Instead the field shows one
//! line from a small curated set: each one is an actual invitation to ask for
//! something, biased towards the vague and the ambitious, which is where an
//! agent is more useful than a search box.
//!
//! Selection is deterministic in the model (a plain index), so captures and
//! tests pin a hint instead of racing a clock or an RNG.

/// The hint set. Keep these short enough to read at a glance and phrased as
/// something you would actually type.
pub const HINTS: &[&str] = &[
    "ask for something above your expectations",
    "ask something vague and let it figure the rest out",
    "describe the bug, not the fix",
    "point at a file and ask what is wrong with it",
    "ask it to do the whole thing, not the first step",
    "ask why this codebase is shaped like this",
    "hand it a failing test and walk away",
    "ask for the version you would not have thought of",
    "say what you want to be true, not what to change",
    "ask it to find what you forgot",
];

/// The hint at `index`, wrapping around so any counter is a valid choice.
pub fn hint(index: usize) -> &'static str {
    HINTS[index % HINTS.len()]
}

/// A starting index that varies between launches, so the hint set is actually
/// discovered over time instead of everyone only ever seeing the first line.
/// Derived from the wall clock rather than an RNG dependency; it only needs to
/// be arbitrary, not unpredictable.
pub fn arbitrary_index() -> usize {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_nanos() as usize)
        .unwrap_or(0)
        % HINTS.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hints_wrap_instead_of_panicking() {
        assert_eq!(hint(0), HINTS[0]);
        assert_eq!(hint(HINTS.len()), HINTS[0]);
        assert_eq!(hint(usize::MAX), HINTS[usize::MAX % HINTS.len()]);
    }

    #[test]
    fn every_hint_fits_a_single_short_line() {
        for hint in HINTS {
            assert!(!hint.is_empty());
            // Long enough to be an invitation, short enough not to wrap in a
            // narrow window (the composer is a single line at rest).
            assert!(
                hint.len() <= 60,
                "hint too long for the field: {hint:?} ({} chars)",
                hint.len()
            );
            // Ghost text is lowercase in this UI, like every other caption.
            assert_eq!(
                *hint,
                hint.to_lowercase(),
                "hints follow the UI's lowercase voice"
            );
            assert!(
                !hint.ends_with('.'),
                "hints are invitations, not sentences: {hint:?}"
            );
        }
    }

    #[test]
    fn hints_are_unique() {
        let mut seen: Vec<&str> = HINTS.to_vec();
        seen.sort_unstable();
        let count = seen.len();
        seen.dedup();
        assert_eq!(seen.len(), count, "duplicate hint");
    }

    #[test]
    fn an_arbitrary_index_is_always_in_range() {
        assert!(arbitrary_index() < HINTS.len());
    }
}
