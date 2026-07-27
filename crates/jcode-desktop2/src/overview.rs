//! The session overview: every live session as a blob in a 2D field.
//!
//! Held Alt zooms the window out of the conversation you are in and into the
//! space of all of them. A session is a circle, its area proportional to how
//! much conversation it holds, so "the big one on the left" is a thing you can
//! point at and remember. Sessions sharing a working directory cluster
//! together, which makes a project legible as a shape rather than as a list of
//! ids.
//!
//! Placement is deterministic: a golden-angle spiral seeds the blobs and a
//! fixed number of relaxation passes push overlaps apart. No randomness and no
//! clock, so the same session set always lays out identically. That is what
//! stops the field from reshuffling under the user's fingers between polls,
//! and what lets the whole thing be tested without a GPU.
//!
//! This module is pure. It owns the sizing, the placement, the focus, and the
//! directional navigation; the renderer and the app only consume it.

use crate::strip::Entry;

/// Smallest blob radius, in logical units. A session with no conversation yet
/// is still a target you have to be able to see and click.
const MIN_RADIUS: f64 = 30.0;
/// Largest blob radius. Capped so one enormous session cannot crowd the field
/// down to specks: the overview is for comparing sessions, not for rendering a
/// truthful bar chart.
const MAX_RADIUS: f64 = 105.0;
/// Breathing room between two blobs, in logical units. Tight: sessions in one
/// project should read as a clutch of eggs, not as scattered planets, and the
/// eye groups by proximity long before it reads a label.
const BLOB_GAP: f64 = 4.0;
/// Room around a cluster. Wider than [`BLOB_GAP`] so two projects still read
/// as two groups, but only just: what separates them is the *contrast* between
/// the two spacings, not the absolute size of either.
const CLUSTER_GAP: f64 = 22.0;
/// Relaxation passes. Enough to separate a realistic field, few enough that
/// layout stays trivially cheap to run every frame.
const RELAX_PASSES: usize = 60;
/// Compaction passes run after relaxation, pulling every circle back toward
/// the group's centre until it is just touching. Relaxation alone only ever
/// pushes apart, so the spiral's initial spread was preserved forever and a
/// sparse cluster stayed sparse however much room it did not need.
const COMPACT_PASSES: usize = 40;
/// How far a compaction pass moves a circle toward the centre, as a fraction
/// of the slack it has. Below 1 so the group settles rather than oscillating
/// between overshooting and being pushed back out.
const COMPACT_RATE: f64 = 0.35;
/// Golden angle: the seeding spiral's turn per item. Gives an even, non-
/// repeating packing without any random numbers.
const GOLDEN_ANGLE: f64 = 2.399_963_229_728_653;
/// Fraction of the shorter side of the field left as a margin after fitting.
const FIT_MARGIN: f64 = 0.03;
/// Half-angle of the cone a directional move searches, in radians. 60 degrees
/// each way: wide enough that a blob which is *mostly* to the right counts,
/// narrow enough that "right" never picks something above you.
const CONE: f64 = std::f64::consts::FRAC_PI_3;

/// The region the field is laid out in: the page inside its margins.
///
/// One definition shared by the renderer and by pointer hit-testing, for the
/// same reason [`crate::layout::Frame`] is: if the two ever disagreed, clicks
/// would land on a different blob than the one under the cursor.
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

impl Dir {
    /// Unit vector in screen space, where y grows downward.
    fn vector(self) -> (f64, f64) {
        match self {
            Self::Left => (-1.0, 0.0),
            Self::Right => (1.0, 0.0),
            Self::Up => (0.0, -1.0),
            Self::Down => (0.0, 1.0),
        }
    }
}

/// One placed session.
#[derive(Clone, Debug, PartialEq)]
pub struct Blob {
    /// Index into the entry list the field was built from.
    pub index: usize,
    pub session_id: String,
    /// Label of the cluster this blob belongs to: the working directory's leaf.
    pub label: String,
    pub center: (f64, f64),
    pub radius: f64,
    pub busy: bool,
    pub focused: bool,
    /// Whether this is the session the window is currently attached to, which
    /// is drawn as the one you zoomed out of.
    pub current: bool,
}

/// A cluster's label anchor: the centroid of its blobs, so the name sits with
/// the group rather than at a grid position the blobs have drifted away from.
#[derive(Clone, Debug, PartialEq)]
pub struct ClusterLabel {
    pub label: String,
    pub center: (f64, f64),
    /// Radius of the cluster's bounding circle, so the renderer can place the
    /// label clear of the blobs instead of on top of them.
    pub radius: f64,
}

/// A laid-out field of blobs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Field {
    pub blobs: Vec<Blob>,
    pub clusters: Vec<ClusterLabel>,
}

impl Field {
    pub fn focused(&self) -> Option<&Blob> {
        self.blobs.iter().find(|blob| blob.focused)
    }

    /// The blob under a logical point, if any. Hit-testing is by distance
    /// rather than by bounding box, because a blob is drawn as a circle and a
    /// click in the corner of its box would land on nothing visible.
    pub fn hit(&self, x: f64, y: f64) -> Option<&Blob> {
        self.blobs
            .iter()
            .filter(|blob| {
                let (dx, dy) = (x - blob.center.0, y - blob.center.1);
                dx * dx + dy * dy <= blob.radius * blob.radius
            })
            // The smallest containing blob wins, so a dot resting on the edge
            // of a giant is still clickable.
            .min_by(|a, b| a.radius.total_cmp(&b.radius))
    }

    /// The session a directional move from `from` should land on.
    ///
    /// Picks the nearest blob within a cone around the direction, scoring by
    /// distance along the axis plus a penalty for drifting off it. Distance
    /// rather than index order, because the field is spatial: "right" has to
    /// mean the thing that looks like it is to the right.
    pub fn neighbor(&self, from: &str, dir: Dir) -> Option<&Blob> {
        let origin = self
            .blobs
            .iter()
            .find(|blob| blob.session_id == from)
            .or_else(|| self.blobs.first())?;
        let (ux, uy) = dir.vector();
        self.blobs
            .iter()
            .filter(|blob| blob.session_id != origin.session_id)
            .filter_map(|blob| {
                let dx = blob.center.0 - origin.center.0;
                let dy = blob.center.1 - origin.center.1;
                let distance = (dx * dx + dy * dy).sqrt();
                if distance <= f64::EPSILON {
                    return None;
                }
                let along = (dx * ux + dy * uy) / distance;
                if along < CONE.cos() {
                    return None;
                }
                // Along-axis distance dominates; the off-axis component is a
                // tiebreak, so two blobs the same distance ahead resolve to
                // the better aligned one.
                let axis = dx * ux + dy * uy;
                let off = (dx * uy - dy * ux).abs();
                Some((blob, axis + off * 0.5))
            })
            .min_by(|(_, a), (_, b)| a.total_cmp(b))
            .map(|(blob, _)| blob)
    }

    /// Session ids in reading order (left to right, top to bottom), for the
    /// Tab cycle. Spatial order rather than list order, so Tab walks the field
    /// the way the eye does.
    pub fn reading_order(&self) -> Vec<&str> {
        let mut order: Vec<&Blob> = self.blobs.iter().collect();
        order.sort_by(|a, b| {
            // Bucket by row so a blob slightly higher than its neighbour does
            // not jump the queue; within a row, read left to right.
            let row = |blob: &Blob| (blob.center.1 / (MIN_RADIUS * 2.0)).floor();
            row(a)
                .total_cmp(&row(b))
                .then(a.center.0.total_cmp(&b.center.0))
        });
        order.iter().map(|blob| blob.session_id.as_str()).collect()
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

/// Blob radius for a session's weight.
///
/// Area, not radius, is proportional to the weight: a circle twice as wide
/// looks four times the session, so scaling the radius linearly would wildly
/// overstate a long conversation. The square root keeps the *ink* honest.
/// `sqrt` of a normalized weight, so the largest session in the field sets the
/// top of the scale and a lone session is always drawn comfortably large.
fn radius_for(weight: f64, heaviest: f64) -> f64 {
    if heaviest <= 0.0 {
        return MIN_RADIUS;
    }
    let normalized = (weight.max(0.0) / heaviest).clamp(0.0, 1.0);
    MIN_RADIUS + (MAX_RADIUS - MIN_RADIUS) * normalized.sqrt()
}

/// Group entries by working-directory leaf, preserving first-appearance order.
///
/// Same rule as the strip, and for the same reason: a field whose clusters
/// reorder between polls is unreadable.
fn cluster_of(entry: &Entry) -> String {
    let Some(dir) = entry.working_dir.as_deref() else {
        return "-".to_string();
    };
    let trimmed = dir.trim_end_matches('/');
    match trimmed.rsplit('/').next() {
        Some(leaf) if !leaf.is_empty() => leaf.to_string(),
        _ => "/".to_string(),
    }
}

/// Push overlapping circles apart until they clear each other by `gap`.
///
/// Shared by the two packing levels (blobs within a cluster, clusters within
/// the field), because "separate these circles" is the same problem at both
/// scales and two copies of it would drift.
fn relax(subjects: &[usize], radii: &[f64], centers: &mut [(f64, f64)], gap: f64) {
    for _ in 0..RELAX_PASSES {
        let mut moved = false;
        for (rank, a) in subjects.iter().enumerate() {
            for b in &subjects[rank + 1..] {
                let wanted = radii[*a] + radii[*b] + gap;
                let dx = centers[*b].0 - centers[*a].0;
                let dy = centers[*b].1 - centers[*a].1;
                let distance = (dx * dx + dy * dy).sqrt();
                if distance >= wanted {
                    continue;
                }
                // Two circles seeded exactly on top of each other have no
                // direction to separate along, so give them a deterministic
                // one rather than dividing by zero.
                let (nx, ny) = if distance <= f64::EPSILON {
                    let angle = GOLDEN_ANGLE * *a as f64;
                    (angle.cos(), angle.sin())
                } else {
                    (dx / distance, dy / distance)
                };
                let push = (wanted - distance) / 2.0;
                centers[*a].0 -= nx * push;
                centers[*a].1 -= ny * push;
                centers[*b].0 += nx * push;
                centers[*b].1 += ny * push;
                moved = true;
            }
        }
        if !moved {
            return;
        }
    }
}

/// Pull circles toward their common centre until they are just touching.
///
/// The other half of [`relax`]. Relaxation resolves overlaps but never
/// reclaims space, so a group seeded on a generous spiral stayed exactly as
/// spread out as it was seeded, however much empty paper sat between its
/// members. Alternating the two settles a group into a tight clutch: pull
/// everything in, push apart whatever now collides, repeat.
fn compact(subjects: &[usize], radii: &[f64], centers: &mut [(f64, f64)], gap: f64) {
    if subjects.len() < 2 {
        return;
    }
    for _ in 0..COMPACT_PASSES {
        let count = subjects.len() as f64;
        let centre = subjects.iter().fold((0.0, 0.0), |acc, index| {
            (
                acc.0 + centers[*index].0 / count,
                acc.1 + centers[*index].1 / count,
            )
        });
        for index in subjects {
            let dx = centre.0 - centers[*index].0;
            let dy = centre.1 - centers[*index].1;
            let distance = (dx * dx + dy * dy).sqrt();
            if distance <= f64::EPSILON {
                continue;
            }
            // Never step past the centre: a circle that overshoots would be
            // pushed back out next pass, and the group would shimmer instead
            // of settling.
            let step = (distance * COMPACT_RATE).min(distance);
            centers[*index].0 += dx / distance * step;
            centers[*index].1 += dy / distance * step;
        }
        // Re-separate whatever the pull just pushed together. This is what
        // makes the result tight rather than merely smaller: the circles end
        // up resting against one another.
        relax(subjects, radii, centers, gap);
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

    // Bucket into clusters, keeping first-appearance order.
    let mut labels: Vec<String> = Vec::new();
    let mut members: Vec<Vec<usize>> = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let label = cluster_of(entry);
        match labels.iter().position(|name| *name == label) {
            Some(at) => members[at].push(index),
            None => {
                labels.push(label);
                members.push(vec![index]);
            }
        }
    }

    let radii: Vec<f64> = entries
        .iter()
        .map(|entry| radius_for(entry.weight, heaviest))
        .collect();

    // Place each cluster's members in the cluster's *own* coordinate space,
    // then place the clusters as rigid bodies. Relaxing every blob against
    // every other in one flat pass instead let members drift between groups
    // until the projects intermingled, which is the one thing the clustering
    // has to prevent.
    let mut placed: Vec<(f64, f64)> = vec![(0.0, 0.0); entries.len()];
    let mut cluster_radius: Vec<f64> = Vec::with_capacity(members.len());
    for group in &members {
        // Largest first, so the heavy blobs take the middle of their cluster
        // and the small ones fill in around them.
        let mut order = group.clone();
        order.sort_by(|a, b| radii[*b].total_cmp(&radii[*a]));
        let mean: f64 = order.iter().map(|i| radii[*i]).sum::<f64>() / order.len() as f64;
        let pitch = (mean * 2.0 + BLOB_GAP) * 0.75;
        for (rank, index) in order.iter().enumerate() {
            let angle = GOLDEN_ANGLE * rank as f64;
            let distance = pitch * (rank as f64).sqrt();
            placed[*index] = (distance * angle.cos(), distance * angle.sin());
        }
        relax(&order, &radii, &mut placed, BLOB_GAP);
        compact(&order, &radii, &mut placed, BLOB_GAP);
        // Recentre on the members' bounding circle so the cluster's own origin
        // is where it looks like it is, which is what the label hangs off.
        let count = order.len() as f64;
        let centroid = order.iter().fold((0.0, 0.0), |acc, index| {
            (
                acc.0 + placed[*index].0 / count,
                acc.1 + placed[*index].1 / count,
            )
        });
        let mut extent: f64 = 0.0;
        for index in &order {
            placed[*index].0 -= centroid.0;
            placed[*index].1 -= centroid.1;
            let (x, y) = placed[*index];
            extent = extent.max((x * x + y * y).sqrt() + radii[*index]);
        }
        cluster_radius.push(extent.max(MIN_RADIUS));
    }

    // Now pack the clusters themselves, as circles of their bounding radius,
    // and carry their members along.
    let mut origins: Vec<(f64, f64)> = (0..members.len())
        .map(|cluster| {
            let angle = GOLDEN_ANGLE * cluster as f64;
            // Seed off this cluster's own size rather than the biggest one in
            // the field: using the maximum spaced every pair as if both were
            // the largest, which pushed a couple of small projects to opposite
            // corners of the page for no reason.
            let spread = cluster_radius[cluster];
            let distance = (spread + CLUSTER_GAP) * (cluster as f64).sqrt();
            (distance * angle.cos(), distance * angle.sin())
        })
        .collect();
    let all_clusters: Vec<usize> = (0..members.len()).collect();
    relax(&all_clusters, &cluster_radius, &mut origins, CLUSTER_GAP);
    compact(&all_clusters, &cluster_radius, &mut origins, CLUSTER_GAP);
    for (cluster, group) in members.iter().enumerate() {
        for index in group {
            placed[*index].0 += origins[cluster].0;
            placed[*index].1 += origins[cluster].1;
        }
    }

    // Fit the whole field into the area: one uniform scale, so the relative
    // sizes (the entire point of the blobs) survive.
    let (x0, y0, x1, y1) = area;
    let margin = ((x1 - x0).min(y1 - y0) * FIT_MARGIN).max(8.0);
    let (fit_w, fit_h) = (
        (x1 - x0 - margin * 2.0).max(1.0),
        (y1 - y0 - margin * 2.0).max(1.0),
    );
    let mut bounds = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for (index, position) in placed.iter().enumerate() {
        bounds.0 = bounds.0.min(position.0 - radii[index]);
        bounds.1 = bounds.1.min(position.1 - radii[index]);
        bounds.2 = bounds.2.max(position.0 + radii[index]);
        bounds.3 = bounds.3.max(position.1 + radii[index]);
    }
    let span = (
        (bounds.2 - bounds.0).max(1.0),
        (bounds.3 - bounds.1).max(1.0),
    );
    // Never scale *up*: a single session blown up to fill a 4K window would
    // read as an error page rather than as one small conversation.
    let scale = (fit_w / span.0).min(fit_h / span.1).min(1.0);
    // The session you zoomed out of goes in the middle of the window, and
    // everything else arranges itself around it. Centring the field's bounding
    // box instead put the current session wherever the packing happened to
    // leave it, so the conversation you were reading slid off under your eye
    // at the exact moment the field appeared. This is the anchor that makes
    // the zoom feel like the window pulling back rather than a screen change.
    let anchor = current
        .and_then(|id| entries.iter().position(|entry| entry.session_id == id))
        .map(|index| placed[index])
        .unwrap_or(((bounds.0 + bounds.2) / 2.0, (bounds.1 + bounds.3) / 2.0));
    let area_center = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    // Anchoring on the current session can hang the far side of a lopsided
    // field off the page, and a session drawn off-screen is one the user
    // cannot reach. So the anchor is a *preference*: honoured while the field
    // still fits, and slid back inside the margins when it does not.
    //
    // Measured from the field's real extent after scaling rather than from a
    // re-projection of the raw bounds: the radii are scaled too, and counting
    // them at full size left the correction short by exactly the margin it was
    // supposed to reclaim.
    let anchored = |position: (f64, f64)| {
        (
            area_center.0 + (position.0 - anchor.0) * scale,
            area_center.1 + (position.1 - anchor.1) * scale,
        )
    };
    let mut extent = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for (index, position) in placed.iter().enumerate() {
        let (cx, cy) = anchored(*position);
        let r = radii[index] * scale;
        extent.0 = extent.0.min(cx - r);
        extent.1 = extent.1.min(cy - r);
        extent.2 = extent.2.max(cx + r);
        extent.3 = extent.3.max(cy + r);
    }
    /// Slide a span back inside its edges, or centre it when it cannot fit.
    fn shift(low: f64, high: f64, edge_low: f64, edge_high: f64) -> f64 {
        if high - low >= edge_high - edge_low {
            // Too big for the page even after fitting: centre the field so the
            // overflow is shared, rather than letting one whole end fall off.
            return (edge_low + edge_high) / 2.0 - (low + high) / 2.0;
        }
        if low < edge_low {
            edge_low - low
        } else if high > edge_high {
            edge_high - high
        } else {
            0.0
        }
    }
    let dx = shift(extent.0, extent.2, x0 + margin, x1 - margin);
    let dy = shift(extent.1, extent.3, y0 + margin, y1 - margin);
    let to_screen = |position: (f64, f64)| {
        let (cx, cy) = anchored(position);
        (cx + dx, cy + dy)
    };

    let blobs: Vec<Blob> = entries
        .iter()
        .enumerate()
        .map(|(index, entry)| Blob {
            index,
            session_id: entry.session_id.clone(),
            label: cluster_of(entry),
            center: to_screen(placed[index]),
            radius: radii[index] * scale,
            busy: entry.busy,
            focused: focus == Some(entry.session_id.as_str()),
            current: current == Some(entry.session_id.as_str()),
        })
        .collect();

    let clusters = members
        .iter()
        .enumerate()
        .map(|(cluster, group)| {
            let count = group.len() as f64;
            let center = group.iter().fold((0.0, 0.0), |acc, index| {
                (
                    acc.0 + blobs[*index].center.0 / count,
                    acc.1 + blobs[*index].center.1 / count,
                )
            });
            let radius = group
                .iter()
                .map(|index| {
                    let blob = &blobs[*index];
                    let dx = blob.center.0 - center.0;
                    let dy = blob.center.1 - center.1;
                    (dx * dx + dy * dy).sqrt() + blob.radius
                })
                .fold(0.0f64, f64::max);
            ClusterLabel {
                label: labels[cluster].clone(),
                center,
                radius,
            }
        })
        .collect();

    Field { blobs, clusters }
}

/// A blob's caption: the human-readable part of a session id.
///
/// Session ids are `session_<name>_<millis>_<hash>`, of which only the name is
/// worth reading. Printing the whole id would make every blob look identical
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

/// A 0..1 breath on a wall clock, for the busy pulse.
///
/// Lives here rather than in the renderer so the sizing of a blob is one
/// function of (blob, time) that a test can evaluate without a GPU.
pub fn breath(now: std::time::Instant, period: f32) -> f64 {
    // Phase from a process-relative clock: the absolute epoch does not matter,
    // only that every blob breathes together.
    let seconds = now.elapsed().as_secs_f32();
    let turn = std::f32::consts::TAU * seconds / period.max(0.01);
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
    /// The escape hatch for the instant-open gesture: Alt opens the field on
    /// the keydown, so a chord like Alt+B has to erase it in the same frame
    /// the letter arrives. Zooming out here would leave the blobs washing
    /// over the composer for a tenth of a second after the user has already
    /// moved on, which reads as lag rather than as an animation.
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

    /// The whole premise: a bigger conversation is a bigger blob.
    #[test]
    fn radius_grows_with_the_transcript() {
        let field = field();
        let by = |id: &str| {
            field
                .blobs
                .iter()
                .find(|blob| blob.session_id == id)
                .unwrap()
                .radius
        };
        assert!(by("a1") > by("a2"), "the heavy session was not the big one");
        assert!(by("a2") > by("a3"));
    }

    /// Area, not radius, tracks the weight: a session ten times the size must
    /// not be drawn ten times as wide.
    #[test]
    fn sizing_is_by_area_not_by_radius() {
        let small = radius_for(10.0, 1000.0);
        let large = radius_for(1000.0, 1000.0);
        assert!(
            large < small * 10.0,
            "radius scaled linearly with the weight"
        );
    }

    /// An empty session still gets a blob big enough to see and click.
    #[test]
    fn a_fresh_session_is_still_a_visible_target() {
        let field = layout(&[entry("solo", "/tmp", 0.0)], Some("solo"), None, AREA);
        assert_eq!(field.blobs.len(), 1);
        assert!(field.blobs[0].radius >= MIN_RADIUS * 0.5);
    }

    /// Blobs must not sit on top of one another, or the field is unreadable
    /// and half the sessions are unclickable.
    #[test]
    fn blobs_do_not_overlap() {
        let field = field();
        for (i, a) in field.blobs.iter().enumerate() {
            for b in &field.blobs[i + 1..] {
                let dx = b.center.0 - a.center.0;
                let dy = b.center.1 - a.center.1;
                let distance = (dx * dx + dy * dy).sqrt();
                assert!(
                    distance >= a.radius + b.radius - 1.0,
                    "{} and {} overlap",
                    a.session_id,
                    b.session_id
                );
            }
        }
    }

    /// The field has to fit the window: a blob drawn off-page is a session the
    /// user cannot reach.
    #[test]
    fn every_blob_fits_inside_the_area() {
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
        for blob in &field.blobs {
            assert!(blob.center.0 - blob.radius >= AREA.0 - 1.0, "{blob:?} left");
            assert!(
                blob.center.0 + blob.radius <= AREA.2 + 1.0,
                "{blob:?} right"
            );
            assert!(blob.center.1 - blob.radius >= AREA.1 - 1.0, "{blob:?} top");
            assert!(
                blob.center.1 + blob.radius <= AREA.3 + 1.0,
                "{blob:?} bottom"
            );
        }
    }

    /// Layout is a pure function of the input: the field must not drift
    /// between polls of the same session set.
    #[test]
    fn layout_is_deterministic() {
        assert_eq!(field(), field());
    }

    /// Sessions in one directory must end up nearer each other than to another
    /// project's, or the clustering says nothing.
    #[test]
    fn sessions_in_a_directory_cluster_together() {
        let field = field();
        let center = |label: &str| {
            field
                .clusters
                .iter()
                .find(|cluster| cluster.label == label)
                .unwrap()
                .center
        };
        let blob = |id: &str| {
            field
                .blobs
                .iter()
                .find(|b| b.session_id == id)
                .unwrap()
                .center
        };
        let distance =
            |a: (f64, f64), b: (f64, f64)| ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt();
        for id in ["a1", "a2", "a3"] {
            assert!(
                distance(blob(id), center("jcode")) < distance(blob(id), center("site")),
                "{id} was nearer the wrong cluster"
            );
        }
    }

    /// Clusters must not intermingle: every blob has to be nearer its own
    /// project's centre than any other's. The first packing relaxed all the
    /// blobs against each other in one flat pass and members drifted between
    /// groups until the projects were visibly interleaved, so this is the
    /// regression the two-level packing exists to prevent.
    #[test]
    fn clusters_stay_disjoint_in_a_crowded_field() {
        let entries: Vec<Entry> = (0..18)
            .map(|n| {
                entry(
                    &format!("s{n}"),
                    &format!("/home/j/proj{}", n % 4),
                    ((n * 7919) % 400) as f64 * 900.0 + 500.0,
                )
            })
            .collect();
        let field = layout(&entries, None, None, AREA);
        let distance =
            |a: (f64, f64), b: (f64, f64)| ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt();
        for blob in &field.blobs {
            let own = field
                .clusters
                .iter()
                .find(|cluster| cluster.label == blob.label)
                .expect("every blob belongs to a cluster");
            let nearest = field
                .clusters
                .iter()
                .min_by(|a, b| {
                    distance(blob.center, a.center).total_cmp(&distance(blob.center, b.center))
                })
                .expect("at least one cluster");
            assert_eq!(
                nearest.label, own.label,
                "{} sat inside the {} cluster",
                blob.session_id, nearest.label
            );
        }
    }

    /// Directional navigation is spatial: moving right must land on something
    /// actually to the right.
    #[test]
    fn a_directional_move_lands_in_that_direction() {
        let field = field();
        for blob in &field.blobs {
            for (dir, ok) in [
                (Dir::Right, (1.0, 0.0)),
                (Dir::Left, (-1.0, 0.0)),
                (Dir::Up, (0.0, -1.0)),
                (Dir::Down, (0.0, 1.0)),
            ] {
                let Some(target) = field.neighbor(&blob.session_id, dir) else {
                    continue;
                };
                let dx = target.center.0 - blob.center.0;
                let dy = target.center.1 - blob.center.1;
                let along = (dx * ok.0 + dy * ok.1) / (dx * dx + dy * dy).sqrt();
                assert!(
                    along >= CONE.cos() - 1e-9,
                    "{dir:?} from {} landed off-axis",
                    blob.session_id
                );
            }
        }
    }

    /// Moving must never be a no-op that looks like a broken key: from any
    /// blob, at least one direction has to go somewhere.
    #[test]
    fn every_blob_can_reach_another() {
        let field = field();
        for blob in &field.blobs {
            let reachable = [Dir::Left, Dir::Right, Dir::Up, Dir::Down]
                .into_iter()
                .filter(|dir| field.neighbor(&blob.session_id, *dir).is_some())
                .count();
            assert!(reachable > 0, "{} is stranded", blob.session_id);
        }
    }

    /// Tab must visit every session exactly once before wrapping, or some
    /// session is unreachable from the keyboard.
    #[test]
    fn tab_order_covers_every_session_once() {
        let field = field();
        let order = field.reading_order();
        assert_eq!(order.len(), field.blobs.len());
        let unique: std::collections::BTreeSet<&str> = order.iter().copied().collect();
        assert_eq!(unique.len(), field.blobs.len());
        let mut at = order[0].to_string();
        for _ in 0..field.blobs.len() {
            at = field.next_in_order(&at, 1).unwrap().to_string();
        }
        assert_eq!(at, order[0], "the cycle did not wrap to the start");
    }

    /// Clicking inside a blob picks it; clicking the paper between blobs picks
    /// nothing rather than the nearest.
    #[test]
    fn hit_testing_is_by_the_drawn_circle() {
        let field = field();
        let blob = field.blobs[0].clone();
        assert_eq!(
            field
                .hit(blob.center.0, blob.center.1)
                .map(|b| &b.session_id),
            Some(&blob.session_id)
        );
        let outside = (
            blob.center.0 + blob.radius * 0.95,
            blob.center.1 + blob.radius * 0.95,
        );
        assert_ne!(
            field.hit(outside.0, outside.1).map(|b| &b.session_id),
            Some(&blob.session_id),
            "the corner of the bounding box was treated as a hit"
        );
    }

    /// The zoom must run to completion and stop, in both directions.
    /// The label is the whole of a blob's identity, so it must be the name a
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
