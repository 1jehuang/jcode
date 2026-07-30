//! Criteria derived from the measured render graph.
//!
//! Split from `harmony.rs` to keep that file inside the size ratchet, and
//! because these two differ in kind from the others: the criteria there inspect
//! colors in isolation or in hand-listed pairs, while these read a *measured*
//! topology (which roles cover area, which roles touch) from
//! [`super::graph`]. See that module for why the graph exists.

use super::{Criterion, DISTINCT_TARGET, Oklab, as_percent, graph};
use crate::palette::{Palette, Role};

/// Whether the colors that actually cover the screen span more than one hue.
///
/// This is the criterion the others could not express. `hue harmony` asks
/// whether the 22 role hues fit a scheme, treating a role that paints 3 cells
/// the same as one that paints 1004. `visual_variety` instead weights each role
/// by measured screen area, so a palette whose dominant roles collapse onto one
/// hue scores badly no matter how tidy its scheme is on paper.
///
/// The case that motivated it: a generated light palette scored 86/100 overall
/// while reading as a single brown-olive wash, because six of its seven
/// highest-area roles sat within 2 degrees of hue 115.
pub(super) fn visual_variety(palette: &Palette) -> Criterion {
    let topology = graph::default_topology();
    let concentration = graph::hue_concentration(palette, &topology);
    let chroma = graph::area_chroma(palette, &topology);

    // A screen dominated by one hue is only a problem when that hue is also
    // washed out. Dracula concentrates hard (0.94) on a *saturated* violet and
    // reads as deliberately tinted; the generated olive palette concentrated
    // just as hard (0.93) on a desaturated olive-gray and read as drab. So the
    // penalty applies to the combination, not to concentration alone.
    //
    // A near-neutral dominant color is the normal, good case: jcode's default
    // `dim` is pure gray, which carries no hue to be monotone about.
    const MONOTONE_CONCENTRATION: f32 = 0.85;
    const TINTED_CHROMA: f32 = 0.05;
    let washed = concentration > MONOTONE_CONCENTRATION
        && chroma > NEUTRAL_AREA_CHROMA
        && chroma < TINTED_CHROMA;

    let score = if washed {
        // Scale with how far into the bad region it sits, so the report can
        // distinguish "slightly muddy" from "one flat color".
        let excess = (concentration - MONOTONE_CONCENTRATION) / (1.0 - MONOTONE_CONCENTRATION);
        (1.0 - excess).clamp(0.0, 0.6)
    } else {
        1.0
    };

    let mut findings = Vec::new();
    if washed {
        let mut dominant: Vec<(Role, f32, f32)> = topology
            .nodes
            .iter()
            .filter_map(|node| {
                let lab = Oklab::from_rgb(palette.rgb(node.role));
                (lab.chroma() >= 0.02).then_some((
                    node.role,
                    topology.area_fraction(node.role),
                    lab.hue_degrees(),
                ))
            })
            .collect();
        dominant.sort_by(|left, right| right.1.total_cmp(&left.1));
        let listed: Vec<String> = dominant
            .iter()
            .take(3)
            .map(|(role, share, hue)| {
                format!(
                    "{} ({:.0}% of screen, hue {hue:.0})",
                    role.key(),
                    share * 100.0
                )
            })
            .collect();
        findings.push(format!(
            "the screen is dominated by one desaturated hue, so it reads as a drab wash \
             (area chroma {chroma:.2}): {}. Either push these toward neutral gray or give \
             them real saturation.",
            listed.join(", ")
        ));
    }

    Criterion {
        name: "visual variety",
        score: as_percent(score),
        weight: 2.0,
        findings,
        // Critical: when the screen is one flat muddy color, color has stopped
        // carrying information, which is a usability failure and not a taste.
        critical: true,
    }
}

/// Area chroma at or below this reads as an intentional neutral (gray) rather
/// than a muddy tint, which is the good case rather than a defect.
const NEUTRAL_AREA_CHROMA: f32 = 0.015;

/// Perceptual separation between roles that actually appear next to each other.
///
/// Complements `distinctness`, which checks a hand-listed set of pairs. This
/// checks the pairs the renderer really places side by side, so a new widget
/// that puts two similar roles adjacent is caught without anyone remembering to
/// extend a list.
pub(super) fn neighbour_separation(palette: &Palette) -> Criterion {
    let topology = graph::default_topology();
    let (mean, worst) = graph::adjacent_separation(palette, &topology);
    let score = (mean / DISTINCT_TARGET).clamp(0.0, 1.0);

    let mut findings = Vec::new();
    if let Some((left, right, distance)) = worst
        && distance < DISTINCT_TARGET * 0.75
    {
        findings.push(format!(
            "{} and {} are rendered next to each other but only {distance:.2} apart",
            left.key(),
            right.key()
        ));
    }

    Criterion {
        name: "neighbour contrast",
        score: as_percent(score),
        weight: 1.5,
        findings,
        critical: false,
    }
}
