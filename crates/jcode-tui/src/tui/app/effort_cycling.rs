//! Effort-ladder cycling with swarm-sentinel exclusion.
//!
//! The full selectable effort ladder ends with Jcode's orchestration sentinels
//! (`swarm`, `swarm-deep`). These are NOT reasoning levels — they activate
//! swarm orchestration mode. Naive modular wrap over the full ladder would
//! cycle `max → swarm → swarm-deep → none`, accidentally activating swarm mode
//! when a user wraps past `max`.
//!
//! Instead, the ladder is split into a **reasoning-only sub-ladder**
//! (`none … max`) and the **swarm sentinels** (`swarm`, `swarm-deep`). The wrap
//! rules are:
//!
//! Direction **up** (+1):
//!   - Reasoning level (not last) → next reasoning level.
//!   - Last reasoning level (max) → first swarm sentinel (swarm).
//!   - Swarm sentinel (not last) → next swarm sentinel (swarm → swarm-deep).
//!   - Last swarm sentinel (swarm-deep) → wrap to first reasoning level (none).
//!
//! Direction **down** (-1):
//!   - Reasoning level (not first) → previous reasoning level.
//!   - First reasoning level (none) → wrap to last reasoning level (max).
//!   - Swarm sentinel (not first) → previous swarm sentinel (swarm-deep → swarm).
//!   - First swarm sentinel (swarm) → last reasoning level (max).
//!
//! This keeps swarm modes reachable by cycling UP past `max` (preserving the
//! existing UI affordance) while ensuring that wrapping back down from
//! `swarm-deep` lands on the first reasoning level, never on an intermediate
//! reasoning rung. Wrapping DOWN from `none` stays within reasoning levels and
//! never jumps to `swarm-deep`.

/// Outcome of [`cycle_effort_index`]: the next effort string and its index in
/// the full ladder, plus whether the position changed.
pub(crate) struct CycledEffort {
    /// Index into the full `efforts` ladder.
    pub index: usize,
    /// The effort string at that index.
    pub effort: &'static str,
    /// `false` when the ladder has a single element (or the logic otherwise
    /// could not move). Callers use this to show "already at max/min".
    pub changed: bool,
}

/// Index used when the current effort is not found in the ladder: the
/// highest reasoning level (or the last rung when the ladder has no
/// reasoning levels). Shared by cycling and `/effort <level>` display so
/// the status bar never falls back to the leftmost rung.
pub(crate) fn default_effort_index(efforts: &[&'static str]) -> Option<usize> {
    if efforts.is_empty() {
        return None;
    }
    let (reasoning, _) = reasoning_split(efforts);
    Some(if reasoning.is_empty() {
        efforts.len() - 1
    } else {
        reasoning.len() - 1
    })
}

/// Compute the next effort in the cycling sequence, excluding swarm sentinels
/// from the reasoning-only wrap group.
///
/// `efforts` is the full ladder (reasoning levels followed by `swarm` /
/// `swarm-deep`). `current` is the currently-selected effort string (or `None`
/// to default to the highest reasoning level). `direction` is `+1` for up /
/// higher, `-1` for down / lower.
pub(crate) fn cycle_effort_index(
    efforts: &[&'static str],
    current: Option<&str>,
    direction: i8,
) -> CycledEffort {
    let (reasoning, swarm) = reasoning_split(efforts);

    let default_index = default_effort_index(efforts).expect("non-empty ladder");

    let current_index = current
        .and_then(|c| efforts.iter().position(|e| *e == c))
        .unwrap_or(default_index);

    let next_index = if direction > 0 {
        cycle_up(current_index, efforts, reasoning, swarm)
    } else {
        cycle_down(current_index, efforts, reasoning, swarm)
    };

    let next_effort = efforts[next_index];
    let changed = next_effort != efforts[current_index];

    CycledEffort {
        index: next_index,
        effort: next_effort,
        changed,
    }
}

/// Split the ladder into (reasoning-only, swarm-sentinels) slices.
/// `swarm` and `swarm-deep` are the only sentinels (see
/// `crate::prompt::is_swarm_effort`), and they always trail the ladder.
fn reasoning_split<'a>(efforts: &'a [&'static str]) -> (&'a [&'static str], &'a [&'static str]) {
    let swarm_count = efforts
        .iter()
        .rev()
        .take_while(|e| crate::prompt::is_swarm_effort(e))
        .count();
    let split = efforts.len() - swarm_count;
    (&efforts[..split], &efforts[split..])
}

/// Up direction: reasoning → swarm → wrap to reasoning.
fn cycle_up(
    current: usize,
    efforts: &[&'static str],
    reasoning: &[&'static str],
    swarm: &[&'static str],
) -> usize {
    let is_swarm = crate::prompt::is_swarm_effort(efforts[current]);
    if is_swarm {
        // Inside the swarm block: advance to next sentinel, or wrap to the
        // first reasoning level after the last sentinel.
        if current + 1 < efforts.len() && crate::prompt::is_swarm_effort(efforts[current + 1]) {
            current + 1
        } else {
            // Past the last swarm sentinel → wrap to first reasoning level.
            if reasoning.is_empty() {
                current // no reasoning levels; stay
            } else {
                0
            }
        }
    } else if reasoning.is_empty() {
        // No reasoning levels at all; wrap within swarm.
        (current + 1) % efforts.len()
    } else {
        let reasoning_idx = reasoning
            .iter()
            .position(|e| std::ptr::eq(*e, efforts[current]))
            .unwrap_or(reasoning.len() - 1);
        if reasoning_idx + 1 < reasoning.len() {
            // Next reasoning level (offset into full ladder).
            reasoning_idx + 1
        } else {
            // At last reasoning level (max) → first swarm sentinel, or wrap
            // to first reasoning if no swarm sentinels exist.
            if swarm.is_empty() { 0 } else { reasoning.len() }
        }
    }
}

/// Down direction: wrap within reasoning; swarm → reasoning.
fn cycle_down(
    current: usize,
    efforts: &[&'static str],
    reasoning: &[&'static str],
    _swarm: &[&'static str],
) -> usize {
    let is_swarm = crate::prompt::is_swarm_effort(efforts[current]);
    if is_swarm {
        // Inside swarm: go to previous sentinel, or to last reasoning level.
        if current > 0 && crate::prompt::is_swarm_effort(efforts[current - 1]) {
            current - 1
        } else {
            // First swarm sentinel → last reasoning level (max).
            if reasoning.is_empty() {
                current
            } else {
                reasoning.len() - 1
            }
        }
    } else if reasoning.is_empty() {
        (current + efforts.len() - 1) % efforts.len()
    } else {
        let reasoning_idx = reasoning
            .iter()
            .position(|e| std::ptr::eq(*e, efforts[current]))
            .unwrap_or(0);
        if reasoning_idx > 0 {
            reasoning_idx - 1
        } else {
            // First reasoning level → wrap to last reasoning level (within
            // the reasoning-only sub-ladder; never jump to swarm-deep).
            reasoning.len() - 1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ladder() -> Vec<&'static str> {
        vec![
            "none",
            "minimal",
            "low",
            "medium",
            "high",
            "xhigh",
            "max",
            "swarm",
            "swarm-deep",
        ]
    }

    #[test]
    fn up_through_reasoning_then_swarm_then_wrap() {
        let ladder = ladder();
        // Start at "none" (index 0), cycle up 9 times — should visit every
        // rung then wrap back to "none".
        let mut cur: &'static str = "none";
        let expected = [
            "minimal",
            "low",
            "medium",
            "high",
            "xhigh",
            "max",
            "swarm",
            "swarm-deep",
            "none",
        ];
        for want in expected {
            let r = cycle_effort_index(&ladder, Some(cur), 1);
            assert_eq!(r.effort, want, "from {cur} up -> {want}");
            assert!(r.changed, "should have changed from {cur}");
            cur = r.effort;
        }
    }

    #[test]
    fn down_through_swarm_then_reasoning_wrap() {
        let ladder = ladder();
        // Start at "swarm-deep" (last), cycle down — should go to swarm, then
        // max, then wrap within reasoning down to none, then wrap to max.
        let mut cur: &'static str = "swarm-deep";
        let expected = [
            "swarm", "max", "xhigh", "high", "medium", "low", "minimal", "none", "max",
        ];
        for want in expected {
            let r = cycle_effort_index(&ladder, Some(cur), -1);
            assert_eq!(r.effort, want, "from {cur} down -> {want}");
            assert!(r.changed, "should have changed from {cur}");
            cur = r.effort;
        }
    }

    #[test]
    fn up_from_max_goes_to_swarm_not_none() {
        let ladder = ladder();
        let r = cycle_effort_index(&ladder, Some("max"), 1);
        assert_eq!(r.effort, "swarm");
        assert!(r.changed);
    }

    #[test]
    fn down_from_none_wraps_to_max_not_swarm_deep() {
        let ladder = ladder();
        let r = cycle_effort_index(&ladder, Some("none"), -1);
        assert_eq!(r.effort, "max");
        assert!(r.changed);
    }

    #[test]
    fn up_from_swarm_deep_wraps_to_none() {
        let ladder = ladder();
        let r = cycle_effort_index(&ladder, Some("swarm-deep"), 1);
        assert_eq!(r.effort, "none");
        assert!(r.changed);
    }

    #[test]
    fn down_from_swarm_goes_to_max() {
        let ladder = ladder();
        let r = cycle_effort_index(&ladder, Some("swarm"), -1);
        assert_eq!(r.effort, "max");
        assert!(r.changed);
    }

    #[test]
    fn default_effort_index_is_highest_reasoning_level() {
        assert_eq!(default_effort_index(&ladder()), Some(6)); // "max"
        // Swarm-only ladder: last rung.
        assert_eq!(default_effort_index(&["swarm", "swarm-deep"]), Some(1));
        assert_eq!(default_effort_index(&[]), None);
    }

    #[test]
    fn single_element_ladder_reports_unchanged() {
        let ladder = vec!["high"];
        let r = cycle_effort_index(&ladder, Some("high"), 1);
        assert!(!r.changed);
    }

    #[test]
    fn no_swarm_sentinels_wraps_within_reasoning() {
        let ladder = vec!["none", "low", "high"];
        let r = cycle_effort_index(&ladder, Some("high"), 1);
        assert_eq!(r.effort, "none");
        let r = cycle_effort_index(&ladder, Some("none"), -1);
        assert_eq!(r.effort, "high");
    }

    #[test]
    fn current_not_in_ladder_defaults_to_max_reasoning() {
        let ladder = ladder();
        let r = cycle_effort_index(&ladder, Some("unknown"), 1);
        // Default is "max" (last reasoning), so up goes to "swarm".
        assert_eq!(r.effort, "swarm");
        assert!(r.changed);
    }
}
