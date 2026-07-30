//! Adjacency- and area-aware palette structure, derived from real frames.
//!
//! # Why a graph
//!
//! The criteria in [`super`] measure colors in isolation (each role against the
//! background) or in a hand-listed set of pairs. That misses the two things a
//! reader actually reacts to:
//!
//! 1. **Which colors touch.** Two roles can be far apart in Oklab and still
//!    clash, or sit adjacent all over the screen and never be compared by a
//!    fixed pair list. The pairs that matter are the ones that *co-occur*.
//! 2. **How much screen each color covers.** A role painting 28 cells dominates
//!    the impression; a role painting 2 barely registers. Scoring all roles
//!    equally lets a palette look monotone while scoring well, which is exactly
//!    what happened to a generated light palette: six of its seven
//!    highest-area roles landed within 2 degrees of hue 115, and no existing
//!    criterion noticed.
//!
//! So this module models a palette as a weighted graph: nodes are roles carrying
//! screen area, edges are measured adjacency between roles. That turns "do these
//! colors interact well" into a question about the graph rather than about
//! individually inspected colors.
//!
//! # Where the graph comes from
//!
//! Node weights and edges are *measured*, not declared: `jcode-tui` renders real
//! frames and records which roles occupy neighbouring cells, and the resulting
//! counts are checked in as [`DEFAULT_TOPOLOGY`]. Declaring adjacency by hand
//! would encode what someone thinks the UI looks like; measuring it encodes what
//! it does look like.

use crate::harmony::Oklab;
use crate::palette::{ALL_ROLES, Palette, Role};

/// One role's share of the rendered frame, in cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeWeight {
    pub role: Role,
    /// Foreground cells painted with this role across the sampled frames.
    pub cells: u32,
}

/// Two roles observed in neighbouring cells, with how often.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Edge {
    pub left: Role,
    pub right: Role,
    /// Times these roles appeared adjacent across the sampled frames.
    pub touches: u32,
}

/// A measured layout: how much area each role covers and which roles touch.
#[derive(Debug, Clone, PartialEq)]
pub struct Topology {
    pub nodes: Vec<NodeWeight>,
    pub edges: Vec<Edge>,
}

impl Topology {
    /// Total sampled foreground area.
    pub fn total_cells(&self) -> u32 {
        self.nodes.iter().map(|node| node.cells).sum()
    }

    /// A role's fraction of the sampled area, 0.0 if it never appeared.
    pub fn area_fraction(&self, role: Role) -> f32 {
        let total = self.total_cells();
        if total == 0 {
            return 0.0;
        }
        self.nodes
            .iter()
            .find(|node| node.role == role)
            .map_or(0.0, |node| node.cells as f32 / total as f32)
    }

    /// Build a topology from per-role cell counts and adjacency counts.
    pub fn from_counts(
        nodes: impl IntoIterator<Item = (Role, u32)>,
        edges: impl IntoIterator<Item = (Role, Role, u32)>,
    ) -> Self {
        Self {
            nodes: nodes
                .into_iter()
                .map(|(role, cells)| NodeWeight { role, cells })
                .collect(),
            edges: edges
                .into_iter()
                .map(|(left, right, touches)| Edge {
                    left,
                    right,
                    touches,
                })
                .collect(),
        }
    }

    /// Fallback used when no measurement is available: every role equally
    /// weighted, no adjacency. Deliberately inert, so a caller without real
    /// data gets the old role-centric behavior rather than a fabricated layout.
    pub fn uniform() -> Self {
        Self {
            nodes: ALL_ROLES
                .iter()
                .copied()
                .map(|role| NodeWeight { role, cells: 1 })
                .collect(),
            edges: Vec::new(),
        }
    }
}

/// The topology measured from jcode's own rendered frames.
///
/// Regenerate with
/// `cargo test -p jcode-tui --lib print_measured_palette_topology -- --ignored --nocapture`
/// after UI changes that shift how much area each role paints.
///
/// The distribution is the point: `dim` covers 77% of painted cells while every
/// semantic role covers under 1%. Any scoring that weights roles equally is
/// therefore describing a screen nobody sees.
pub fn default_topology() -> Topology {
    Topology::from_counts(
        [
            (Role::Dim, 1004),
            (Role::HeaderName, 137),
            (Role::System, 63),
            (Role::UserText, 42),
            (Role::Pending, 41),
            (Role::User, 9),
            (Role::Error, 3),
            (Role::Warning, 1),
        ],
        [
            (Role::Dim, Role::HeaderName, 10),
            (Role::Error, Role::User, 3),
            (Role::HeaderName, Role::UserText, 3),
            (Role::Pending, Role::User, 2),
            (Role::User, Role::Warning, 1),
        ],
    )
}

/// How concentrated the *visible* color is around a single hue, 0.0 to 1.0.
///
/// This is the circular mean resultant length over chromatic roles, weighted by
/// screen area: 1.0 means every colored pixel shares one hue (a monotone wash),
/// 0.0 means hues are spread evenly around the wheel.
///
/// Area weighting is the entire point. jcode's default palette measures 0.10
/// here while a generated palette that scored *higher* on every existing
/// criterion measured 0.36, because its high-area roles had collapsed onto one
/// hue. Unweighted hue analysis cannot see that difference.
pub fn hue_concentration(palette: &Palette, topology: &Topology) -> f32 {
    let mut x = 0.0f32;
    let mut y = 0.0f32;
    let mut weight = 0.0f32;
    for node in &topology.nodes {
        let lab = Oklab::from_rgb(palette.rgb(node.role));
        // Neutrals carry no hue, so including them would dilute the measure
        // toward 0 and hide a genuine collapse.
        if lab.chroma() < NEUTRAL_CHROMA {
            continue;
        }
        let radians = lab.hue_degrees().to_radians();
        let w = node.cells as f32;
        x += radians.cos() * w;
        y += radians.sin() * w;
        weight += w;
    }
    if weight <= 0.0 {
        return 0.0;
    }
    ((x * x + y * y).sqrt() / weight).clamp(0.0, 1.0)
}

/// Below this chroma a color reads as gray and carries no usable hue.
const NEUTRAL_CHROMA: f32 = 0.04;

/// Area-weighted mean chroma: how colorful the screen actually is.
///
/// Pairs with [`hue_concentration`] to tell apart two very different palettes
/// that both concentrate on one hue. Dracula measures 0.94 concentration but
/// 0.091 area chroma, so its dominant color is a *saturated* violet and the
/// screen reads as deliberately tinted. A generated olive palette measured 0.93
/// concentration with 0.022 area chroma, so its dominant color was a desaturated
/// olive-gray and the screen read as a drab wash. Concentration alone cannot
/// distinguish those; concentration plus chroma can.
pub fn area_chroma(palette: &Palette, topology: &Topology) -> f32 {
    let mut chroma = 0.0f32;
    let mut weight = 0.0f32;
    for node in &topology.nodes {
        chroma += Oklab::from_rgb(palette.rgb(node.role)).chroma() * node.cells as f32;
        weight += node.cells as f32;
    }
    if weight <= 0.0 { 0.0 } else { chroma / weight }
}

/// Perceptual separation between roles that actually touch on screen, weighted
/// by how often they touch.
///
/// Returns the area-weighted mean distance and the worst offending edge. Unlike
/// the fixed `MUST_DISTINGUISH` list, this covers whatever the UI really places
/// side by side, so a new widget that puts two similar roles next to each other
/// is caught without anyone remembering to add a pair.
pub fn adjacent_separation(
    palette: &Palette,
    topology: &Topology,
) -> (f32, Option<(Role, Role, f32)>) {
    let mut total = 0.0f32;
    let mut weight = 0.0f32;
    let mut worst: Option<(Role, Role, f32)> = None;
    for edge in &topology.edges {
        if edge.left == edge.right {
            continue;
        }
        let distance = Oklab::from_rgb(palette.rgb(edge.left))
            .distance(Oklab::from_rgb(palette.rgb(edge.right)));
        let w = edge.touches as f32;
        total += distance * w;
        weight += w;
        if worst.is_none_or(|(_, _, previous)| distance < previous) {
            worst = Some((edge.left, edge.right, distance));
        }
    }
    if weight <= 0.0 {
        return (1.0, None);
    }
    (total / weight, worst)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn two_role_topology(left: Role, right: Role) -> Topology {
        Topology::from_counts([(left, 10), (right, 10)], [(left, right, 5)])
    }

    #[test]
    fn uniform_topology_has_no_edges_and_equal_weights() {
        let topology = Topology::uniform();
        assert_eq!(topology.nodes.len(), ALL_ROLES.len());
        assert!(topology.edges.is_empty());
        let share = topology.area_fraction(Role::User);
        assert!((share - 1.0 / ALL_ROLES.len() as f32).abs() < 1e-6);
    }

    #[test]
    fn area_fraction_reflects_measured_cells() {
        let topology = Topology::from_counts([(Role::User, 30), (Role::Ai, 10)], []);
        assert!((topology.area_fraction(Role::User) - 0.75).abs() < 1e-6);
        assert!((topology.area_fraction(Role::Ai) - 0.25).abs() < 1e-6);
        // A role that never rendered contributes nothing.
        assert_eq!(topology.area_fraction(Role::Error), 0.0);
    }

    /// The measure that the old criteria could not express: a palette whose
    /// large-area roles share a hue must score high concentration even when its
    /// small-area roles are spread out.
    #[test]
    fn hue_concentration_sees_area_weighted_collapse() {
        let mut palette = Palette::default();
        // Three big roles all olive, one tiny role far away in hue.
        palette.set(Role::User, (110, 120, 40));
        palette.set(Role::Ai, (120, 130, 45));
        palette.set(Role::Warning, (115, 125, 42));
        palette.set(Role::Error, (60, 90, 220));

        let collapsed = Topology::from_counts(
            [
                (Role::User, 30),
                (Role::Ai, 30),
                (Role::Warning, 30),
                (Role::Error, 1),
            ],
            [],
        );
        let spread = Topology::from_counts(
            [
                (Role::User, 1),
                (Role::Ai, 1),
                (Role::Warning, 1),
                (Role::Error, 90),
            ],
            [],
        );

        let collapsed_score = hue_concentration(&palette, &collapsed);
        assert!(
            collapsed_score > 0.9,
            "three same-hue roles covering the screen should read as monotone, got {collapsed_score:.2}"
        );

        // The same four colors, but now the odd-hue role owns nearly all of the
        // area. That is also a single-hue screen, so it must also read as
        // concentrated: the measure follows *area*, not the number of roles.
        // (This is the case a role-counting metric gets backwards, reporting
        // "3 of 4 roles agree" regardless of what the user actually sees.)
        let spread_score = hue_concentration(&palette, &spread);
        assert!(
            spread_score > 0.9,
            "one dominant hue is still monotone, got {spread_score:.2}"
        );

        // And a genuinely mixed screen must read as spread.
        let mixed = Topology::from_counts(
            [
                (Role::User, 25),
                (Role::Ai, 25),
                (Role::Warning, 25),
                (Role::Error, 75),
            ],
            [],
        );
        let mixed_score = hue_concentration(&palette, &mixed);
        assert!(
            mixed_score < collapsed_score,
            "mixing a second hue into real area must lower concentration \
             ({mixed_score:.2} vs {collapsed_score:.2})"
        );
    }

    #[test]
    fn hue_concentration_ignores_neutrals() {
        let mut palette = Palette::default();
        palette.set(Role::User, (200, 60, 60));
        palette.set(Role::Dim, (128, 128, 128));
        // A huge neutral area must not dilute the hue reading, or a gray-heavy
        // UI would always look "spread" no matter how monotone its accents are.
        let topology = Topology::from_counts([(Role::User, 5), (Role::Dim, 500)], []);
        assert!(hue_concentration(&palette, &topology) > 0.95);
    }

    #[test]
    fn adjacent_separation_flags_touching_lookalikes() {
        let mut palette = Palette::default();
        palette.set(Role::User, (100, 140, 200));
        palette.set(Role::Ai, (104, 143, 203));
        let (mean, worst) = adjacent_separation(&palette, &two_role_topology(Role::User, Role::Ai));
        assert!(mean < 0.05, "near-identical neighbours should score low");
        let (left, right, distance) = worst.expect("a worst edge exists");
        assert_eq!((left, right), (Role::User, Role::Ai));
        assert!(distance < 0.05);
    }

    #[test]
    fn adjacent_separation_is_neutral_without_measured_edges() {
        // No adjacency data must not be reported as a failure; it is an absence
        // of evidence, so the score is perfect and there is no offender.
        let palette = Palette::default();
        let (mean, worst) = adjacent_separation(&palette, &Topology::uniform());
        assert_eq!(mean, 1.0);
        assert!(worst.is_none());
    }

    #[test]
    fn adjacent_separation_weights_frequent_contacts_more() {
        let mut palette = Palette::default();
        palette.set(Role::User, (100, 140, 200));
        palette.set(Role::Ai, (104, 143, 203)); // clashing pair
        palette.set(Role::Success, (60, 200, 90));
        palette.set(Role::Error, (220, 70, 70)); // well separated pair

        let rare_clash = Topology::from_counts(
            [
                (Role::User, 1),
                (Role::Ai, 1),
                (Role::Success, 1),
                (Role::Error, 1),
            ],
            [(Role::User, Role::Ai, 1), (Role::Success, Role::Error, 100)],
        );
        let common_clash = Topology::from_counts(
            [
                (Role::User, 1),
                (Role::Ai, 1),
                (Role::Success, 1),
                (Role::Error, 1),
            ],
            [(Role::User, Role::Ai, 100), (Role::Success, Role::Error, 1)],
        );

        let (rare, _) = adjacent_separation(&palette, &rare_clash);
        let (common, _) = adjacent_separation(&palette, &common_clash);
        assert!(
            common < rare,
            "a clash that happens constantly should score worse than a rare one \
             ({common:.3} vs {rare:.3})"
        );
    }
}
