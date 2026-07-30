//! Objective palette-harmony measurement.
//!
//! "Does this palette look good?" is subjective, but most of what makes a
//! terminal palette feel *wrong* is measurable. This module scores a
//! [`Palette`](crate::palette::Palette) on independent, individually
//! explainable criteria and reports both a 0-100 score and the specific
//! offenders, so `/colors harmony` can tell a user what to fix rather than
//! just grading them.
//!
//! Criteria (each 0-100, then weighted):
//!
//! - **Readability**: APCA-style lightness contrast of every foreground role
//!   against the terminal background. Unreadable text is the single worst
//!   palette failure, so it carries the most weight.
//! - **Distinctness**: minimum perceptual (oklab) distance between roles that
//!   must be told apart at a glance (`success`/`error`, `user`/`ai`, ...).
//!   Colors that collide destroy the palette's information value.
//! - **Hue harmony**: how well the hues fit a recognized color-theory
//!   relationship (analogous / complementary / triadic / split-complementary)
//!   rather than being scattered arbitrarily around the wheel.
//! - **Chroma coherence**: saturation consistency. Mixing neon and pastel in
//!   one palette reads as unintentional.
//! - **Colorblind safety**: distinctness re-measured under deuteranopia and
//!   protanopia simulation, which catches the classic red/green failure.
//!
//! All math is in Oklab, a perceptually uniform space, so "distance" and
//! "lightness" match what the eye actually reports.

use crate::palette::{ALL_ROLES, Palette, Role, to_hex};

mod generate;
pub mod graph;
mod measured;
pub use generate::generate_from_seed;
pub use graph::{Topology, adjacent_separation, hue_concentration};
use measured::{neighbour_separation, visual_variety};

/// A color in the Oklab perceptual space.
///
/// `l` is perceived lightness in 0..1, `a`/`b` are the green-red and
/// blue-yellow opponent axes (roughly -0.4..0.4).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Oklab {
    pub l: f32,
    pub a: f32,
    pub b: f32,
}

fn srgb_to_linear(channel: u8) -> f32 {
    let c = channel as f32 / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(channel: f32) -> u8 {
    let c = channel.clamp(0.0, 1.0);
    let encoded = if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (encoded.clamp(0.0, 1.0) * 255.0).round() as u8
}

impl Oklab {
    /// Convert sRGB to Oklab (Björn Ottosson's transform).
    pub fn from_rgb((r, g, b): (u8, u8, u8)) -> Self {
        let (r, g, b) = (srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b));
        let l = 0.412_221_47 * r + 0.536_332_5 * g + 0.051_445_995 * b;
        let m = 0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b;
        let s = 0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_5 * b;
        let (l, m, s) = (l.cbrt(), m.cbrt(), s.cbrt());
        Self {
            l: 0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
            a: 1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
            b: 0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
        }
    }

    /// Convert back to sRGB, reducing chroma until the color fits the gamut.
    ///
    /// Naive per-channel clamping shifts hue and lightness, which is how a
    /// requested "vivid green" silently becomes a different color. Scaling
    /// chroma down instead preserves hue and lightness, which is what the
    /// generator and the literal remapper both rely on.
    pub fn to_rgb(self) -> (u8, u8, u8) {
        let mut candidate = self;
        for _ in 0..24 {
            if candidate.in_gamut() {
                return candidate.to_rgb_clamped();
            }
            candidate.a *= 0.92;
            candidate.b *= 0.92;
        }
        candidate.to_rgb_clamped()
    }

    /// Whether this color survives a round trip to sRGB without clipping.
    fn in_gamut(self) -> bool {
        let (r, g, b) = self.to_linear_rgb();
        (-0.001..=1.001).contains(&r)
            && (-0.001..=1.001).contains(&g)
            && (-0.001..=1.001).contains(&b)
    }

    fn to_linear_rgb(self) -> (f32, f32, f32) {
        let l = self.l + 0.396_337_78 * self.a + 0.215_803_76 * self.b;
        let m = self.l - 0.105_561_346 * self.a - 0.063_854_17 * self.b;
        let s = self.l - 0.089_484_18 * self.a - 1.291_485_5 * self.b;
        let (l, m, s) = (l * l * l, m * m * m, s * s * s);
        (
            4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s,
            -1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s,
            -0.004_196_086 * l - 0.703_418_6 * m + 1.707_614_7 * s,
        )
    }

    fn to_rgb_clamped(self) -> (u8, u8, u8) {
        let (r, g, b) = self.to_linear_rgb();
        (linear_to_srgb(r), linear_to_srgb(g), linear_to_srgb(b))
    }

    /// Perceptual distance (Euclidean in Oklab).
    pub fn distance(self, other: Self) -> f32 {
        let dl = self.l - other.l;
        let da = self.a - other.a;
        let db = self.b - other.b;
        (dl * dl + da * da + db * db).sqrt()
    }

    /// Chroma (colorfulness): distance from the neutral axis.
    pub fn chroma(self) -> f32 {
        (self.a * self.a + self.b * self.b).sqrt()
    }

    /// Hue angle in degrees (0..360). Meaningless for near-neutral colors.
    pub fn hue_degrees(self) -> f32 {
        let degrees = self.b.atan2(self.a).to_degrees();
        if degrees < 0.0 {
            degrees + 360.0
        } else {
            degrees
        }
    }
}

/// One scored criterion with human-readable findings.
#[derive(Debug, Clone, PartialEq)]
pub struct Criterion {
    pub name: &'static str,
    /// 0-100, higher is better.
    pub score: u8,
    /// Relative weight in the overall score.
    pub weight: f32,
    /// Specific problems found, best-first. Empty means nothing to fix.
    pub findings: Vec<String>,
    /// Whether this criterion is a hard usability requirement rather than a
    /// matter of taste. Only critical criteria can single-handedly drag the
    /// overall score down: unreadable text is a defect, while an unusual hue
    /// scheme is a legitimate style choice (Solarized breaks textbook hue
    /// rules on purpose and is still widely loved).
    pub critical: bool,
}

/// A full harmony report for a palette.
#[derive(Debug, Clone, PartialEq)]
pub struct HarmonyReport {
    /// Weighted overall score, 0-100.
    pub score: u8,
    /// Best-fitting color-theory scheme for the palette's hues.
    pub scheme: &'static str,
    pub criteria: Vec<Criterion>,
}

impl HarmonyReport {
    /// A short verbal grade for the overall score.
    pub fn grade(&self) -> &'static str {
        match self.score {
            90..=100 => "excellent",
            75..=89 => "good",
            60..=74 => "fair",
            40..=59 => "poor",
            _ => "broken",
        }
    }

    /// Highest-value fixes across all criteria, worst criterion first.
    pub fn top_findings(&self, limit: usize) -> Vec<String> {
        let mut criteria: Vec<&Criterion> = self
            .criteria
            .iter()
            .filter(|criterion| !criterion.findings.is_empty())
            .collect();
        criteria.sort_by_key(|criterion| criterion.score);
        criteria
            .iter()
            .flat_map(|criterion| criterion.findings.iter().cloned())
            .take(limit)
            .collect()
    }
}

/// Role pairs that a user must be able to distinguish instantly. Confusing
/// any of these actively misleads (e.g. mistaking an error for a success).
pub(crate) const MUST_DISTINGUISH: &[(Role, Role)] = &[
    (Role::Success, Role::Error),
    (Role::Success, Role::Warning),
    (Role::Warning, Role::Error),
    (Role::User, Role::Ai),
    (Role::Accent, Role::System),
    (Role::Info, Role::Success),
    (Role::Queued, Role::Asap),
    // Both are status indicators shown in the same column, so a collision here
    // makes the status unreadable. Visual inspection of generated palettes
    // caught them landing on the same hue, which no test had covered.
    (Role::System, Role::Queued),
];
// `user`/`info` and `ai`/`accent` are deliberately *not* listed: Dracula and
// other loved palettes make those similar on purpose, and they never appear in
// the same column, so requiring separation would flag good palettes.

// Note on omissions: `dim`/`tool` are intentionally *both* low-emphasis grays
// in nearly every real palette (Solarized, Nord), so demanding they differ
// would penalize good palettes for doing the right thing.

/// Minimum acceptable oklab distance for a must-distinguish pair.
pub(crate) const DISTINCT_TARGET: f32 = 0.20;

/// Minimum acceptable lightness contrast for body text against the terminal
/// background, in oklab lightness units. ~0.40 corresponds to comfortably
/// readable text; below ~0.25 is a strain.
pub(crate) const CONTRAST_TARGET: f32 = 0.40;

fn ratio_score(actual: f32, target: f32) -> f32 {
    (actual / target).clamp(0.0, 1.0)
}

fn as_percent(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 100.0).round() as u8
}

/// Combine per-item sub-scores into a criterion score.
///
/// A plain mean lets a single unreadable role or a single colliding pair hide
/// behind twenty fine ones, which is exactly the failure a user cares about.
/// Weighting the worst item heavily keeps "one thing is badly broken" visible
/// in the score while still rewarding overall quality.
fn aggregate(scores: &[f32]) -> f32 {
    if scores.is_empty() {
        return 1.0;
    }
    let mean = scores.iter().sum::<f32>() / scores.len() as f32;
    let worst = scores.iter().copied().fold(f32::MAX, f32::min);
    0.4 * mean + 0.6 * worst
}

/// Score `palette` against the given background lightness.
///
/// `background` is the terminal background color; dark and light terminals
/// legitimately need different palettes, so readability is always judged
/// against the real background rather than an assumed black.
pub fn analyze(palette: &Palette, background: (u8, u8, u8)) -> HarmonyReport {
    let criteria = vec![
        readability(palette, background),
        distinctness(palette),
        hue_harmony(palette).0,
        chroma_coherence(palette),
        colorblind_safety(palette),
        visual_variety(palette),
        neighbour_separation(palette),
    ];
    let scheme = hue_harmony(palette).1;

    let total_weight: f32 = criteria.iter().map(|criterion| criterion.weight).sum();
    let weighted: f32 = criteria
        .iter()
        .map(|criterion| criterion.score as f32 * criterion.weight)
        .sum();
    // Only hard usability failures pull the overall score toward the worst
    // dimension. Taste criteria (hue scheme, chroma spread) contribute through
    // the weighted mean only, so a deliberately unconventional but perfectly
    // usable palette still scores well.
    let worst_critical = criteria
        .iter()
        .filter(|criterion| criterion.critical)
        .map(|criterion| criterion.score as f32)
        .fold(f32::MAX, f32::min);
    let score = if total_weight > 0.0 {
        let mean = weighted / total_weight;
        if worst_critical.is_finite() {
            (0.75 * mean + 0.25 * worst_critical).round() as u8
        } else {
            mean.round() as u8
        }
    } else {
        0
    };

    HarmonyReport {
        score,
        scheme,
        criteria,
    }
}

/// Convenience wrapper: analyze the active palette against the active theme's
/// conventional background.
pub fn analyze_active() -> HarmonyReport {
    let background = if crate::theme_mode::is_light_theme() {
        (255, 255, 255)
    } else {
        (18, 18, 18)
    };
    analyze(&crate::palette::palette(), background)
}

fn readability(palette: &Palette, background: (u8, u8, u8)) -> Criterion {
    let bg = Oklab::from_rgb(background);
    let mut findings = Vec::new();
    let mut worst_scores = Vec::new();

    for role in ALL_ROLES.iter().copied() {
        if role.is_background() {
            // Backgrounds are judged by how little they fight the terminal
            // background, not by contrast against it.
            let distance = Oklab::from_rgb(palette.rgb(role)).distance(bg);
            if distance > 0.45 {
                findings.push(format!(
                    "{} ({}) is a jarringly strong background; keep panel backgrounds close to the terminal background",
                    role.key(),
                    to_hex(palette.rgb(role))
                ));
                worst_scores.push(0.5);
            } else {
                worst_scores.push(1.0);
            }
            continue;
        }

        let contrast = (Oklab::from_rgb(palette.rgb(role)).l - bg.l).abs();
        // `dim` and `pending` are intentionally low-contrast, so hold them to a
        // reduced target instead of flagging them by design.
        let target = if matches!(role, Role::Dim | Role::Pending | Role::Border) {
            CONTRAST_TARGET * 0.55
        } else {
            CONTRAST_TARGET
        };
        let score = ratio_score(contrast, target);
        if score < 0.85 {
            findings.push(format!(
                "{} ({}) has low contrast against the background (lightness delta {:.2}, want {:.2})",
                role.key(),
                to_hex(palette.rgb(role)),
                contrast,
                target
            ));
        }
        worst_scores.push(score);
    }

    let mean = aggregate(&worst_scores);
    findings.sort();
    Criterion {
        name: "readability",
        score: as_percent(mean),
        weight: 3.0,
        findings,
        critical: true,
    }
}

fn distinctness(palette: &Palette) -> Criterion {
    let mut findings = Vec::new();
    let mut scores = Vec::new();
    for (left, right) in MUST_DISTINGUISH.iter().copied() {
        let distance =
            Oklab::from_rgb(palette.rgb(left)).distance(Oklab::from_rgb(palette.rgb(right)));
        let score = ratio_score(distance, DISTINCT_TARGET);
        if score < 0.85 {
            findings.push(format!(
                "{} ({}) and {} ({}) look too similar (distance {:.2}, want {:.2})",
                left.key(),
                to_hex(palette.rgb(left)),
                right.key(),
                to_hex(palette.rgb(right)),
                distance,
                DISTINCT_TARGET
            ));
        }
        scores.push(score);
    }
    let mean = aggregate(&scores);
    Criterion {
        name: "distinctness",
        score: as_percent(mean),
        weight: 2.0,
        findings,
        critical: true,
    }
}

/// Recognized hue relationships, as offsets from a base hue.
const SCHEMES: &[(&str, &[f32])] = &[
    ("monochromatic", &[0.0]),
    ("analogous", &[0.0, 30.0, -30.0, 60.0, -60.0]),
    ("complementary", &[0.0, 180.0]),
    ("split-complementary", &[0.0, 150.0, 210.0]),
    ("triadic", &[0.0, 120.0, 240.0]),
    ("tetradic", &[0.0, 90.0, 180.0, 270.0]),
];

pub(crate) fn hue_delta(a: f32, b: f32) -> f32 {
    let diff = (a - b).abs() % 360.0;
    diff.min(360.0 - diff)
}

fn hue_harmony(palette: &Palette) -> (Criterion, &'static str) {
    // Only chromatic roles carry hue information; neutrals (grays, near-white
    // text) belong to every scheme and would wash out the measurement.
    let hues: Vec<(Role, f32)> = ALL_ROLES
        .iter()
        .copied()
        .filter_map(|role| {
            let lab = Oklab::from_rgb(palette.rgb(role));
            (lab.chroma() >= 0.04).then_some((role, lab.hue_degrees()))
        })
        .collect();

    if hues.len() < 2 {
        return (
            Criterion {
                name: "hue harmony",
                score: 100,
                weight: 2.0,
                findings: Vec::new(),
                critical: false,
            },
            "monochromatic",
        );
    }

    // Try every scheme anchored on every present hue, and keep the best fit.
    /// Best scheme fit so far: mean hue deviation, scheme name, and each
    /// chromatic role's deviation from its nearest scheme target.
    struct Fit {
        mean_deviation: f32,
        scheme: &'static str,
        deviations: Vec<(Role, f32)>,
    }

    let mut best: Option<Fit> = None;
    for (name, offsets) in SCHEMES.iter().copied() {
        for (_, anchor) in hues.iter().copied() {
            let targets: Vec<f32> = offsets
                .iter()
                .map(|offset| (anchor + offset).rem_euclid(360.0))
                .collect();
            let mut deviations = Vec::new();
            for (role, hue) in hues.iter().copied() {
                let nearest = targets
                    .iter()
                    .map(|target| hue_delta(hue, *target))
                    .fold(f32::MAX, f32::min);
                deviations.push((role, nearest));
            }
            let mean = deviations
                .iter()
                .map(|(_, deviation)| *deviation)
                .sum::<f32>()
                / deviations.len() as f32;
            if best
                .as_ref()
                .is_none_or(|previous| mean < previous.mean_deviation)
            {
                best = Some(Fit {
                    mean_deviation: mean,
                    scheme: name,
                    deviations,
                });
            }
        }
    }

    // `SCHEMES` is non-empty so a fit is always found, but express that as a
    // fallback rather than an assertion: this runs while rendering a user's
    // palette, and no scoring detail is worth risking a panic there. A missing
    // fit degrades to "no hue structure detected".
    let Some(Fit {
        mean_deviation,
        scheme,
        deviations,
    }) = best
    else {
        return (
            Criterion {
                name: "hue harmony",
                score: 100,
                weight: 2.0,
                findings: Vec::new(),
                critical: false,
            },
            "unknown",
        );
    };
    // A 45 degree mean deviation is where hues stop reading as related at all;
    // real palettes routinely sit 15-25 degrees off a textbook scheme.
    let score = 1.0 - (mean_deviation / 45.0).clamp(0.0, 1.0);
    let mut outliers: Vec<(Role, f32)> = deviations
        .into_iter()
        .filter(|(_, deviation)| *deviation > 35.0)
        .collect();
    outliers.sort_by(|left, right| right.1.total_cmp(&left.1));
    let findings = outliers
        .into_iter()
        .map(|(role, deviation)| {
            format!(
                "{} ({}) sits {deviation:.0}° off the palette's {scheme} hue structure",
                role.key(),
                to_hex(palette.rgb(role))
            )
        })
        .collect();

    (
        Criterion {
            name: "hue harmony",
            score: as_percent(score),
            weight: 1.5,
            findings,
            critical: false,
        },
        scheme,
    )
}

fn chroma_coherence(palette: &Palette) -> Criterion {
    // Grade the palette's *accent* colors. Text and background roles are
    // near-neutral by design, and including them would dilute an oversaturated
    // accent set behind a wall of grays.
    let chromas: Vec<(Role, f32)> = ALL_ROLES
        .iter()
        .copied()
        .filter(|role| !role.is_background())
        .map(|role| (role, Oklab::from_rgb(palette.rgb(role)).chroma()))
        .filter(|(_, chroma)| *chroma >= 0.04)
        .collect();
    if chromas.len() < 2 {
        return Criterion {
            name: "chroma coherence",
            score: 100,
            weight: 1.5,
            findings: Vec::new(),
            critical: false,
        };
    }

    let mean = chromas.iter().map(|(_, c)| *c).sum::<f32>() / chromas.len() as f32;
    let variance = chromas
        .iter()
        .map(|(_, c)| (c - mean) * (c - mean))
        .sum::<f32>()
        / chromas.len() as f32;
    let deviation = variance.sqrt();
    // Judge the spread *relative* to the palette's own saturation level: a
    // vivid palette is allowed vivid variation, while a pastel palette should
    // stay pastel. An absolute threshold mislabeled saturated-but-coherent
    // palettes (Gruvbox, Dracula) as incoherent.
    let relative = if mean > 0.0 { deviation / mean } else { 0.0 };
    let spread_score = 1.0 - (relative / 1.4).clamp(0.0, 1.0);

    // Intensity matters as much as consistency. Terminal palettes live in a
    // comfortable chroma band: below it everything is washed-out gray, above it
    // (fully saturated neon) long reading sessions are fatiguing and the colors
    // stop reading as a designed set. Every palette people actually adopt sits
    // inside this band, so grading it catches "technically distinct but
    // unbearable" palettes that spread alone rates highly.
    // Bounds calibrated against palettes people actually adopt: Nord and
    // Solarized sit near the low end, Dracula and Gruvbox near the high end,
    // and fully saturated sRGB primaries (chroma ~0.25+) fall outside.
    const COMFORTABLE_CHROMA: std::ops::RangeInclusive<f32> = 0.045..=0.20;
    let intensity_of = |chroma: f32| {
        if COMFORTABLE_CHROMA.contains(&chroma) {
            1.0
        } else if chroma < *COMFORTABLE_CHROMA.start() {
            (chroma / COMFORTABLE_CHROMA.start()).clamp(0.0, 1.0)
        } else {
            (1.0 - (chroma - COMFORTABLE_CHROMA.end()) / 0.05).clamp(0.0, 1.0)
        }
    };
    // Score intensity per role, not on the mean: a handful of screaming neon
    // roles must not be averaged away by the well-behaved ones they sit next to.
    let intensities: Vec<f32> = chromas
        .iter()
        .map(|(_, chroma)| intensity_of(*chroma))
        .collect();
    let intensity_score = aggregate(&intensities);
    let score = spread_score.min(intensity_score);

    let mut findings = Vec::new();
    if intensity_score < 0.85 {
        findings.push(if mean >= *COMFORTABLE_CHROMA.end() {
            format!(
                "the palette is oversaturated overall (mean chroma {mean:.2}); fully saturated \
                 colors are fatiguing to read for long stretches"
            )
        } else {
            format!(
                "the palette is washed out overall (mean chroma {mean:.2}); colors barely read as \
                 colors"
            )
        });
    }
    for (role, chroma) in chromas {
        if mean > 0.0 && (chroma - mean).abs() / mean > 1.1 {
            let direction = if chroma > mean {
                "much more saturated"
            } else {
                "much less saturated"
            };
            findings.push(format!(
                "{} ({}) is {direction} than the rest of the palette",
                role.key(),
                to_hex(palette.rgb(role))
            ));
        }
    }

    Criterion {
        name: "chroma coherence",
        score: as_percent(score),
        weight: 1.5,
        findings,
        // Critical: a palette far outside the comfortable saturation band is
        // physically tiring to read for hours, which is a usability defect
        // rather than a style preference.
        critical: true,
    }
}

/// Simulate common color vision deficiencies with the Brettel-style
/// linear-RGB projections (the standard approximation used by accessibility
/// tooling).
pub(crate) fn simulate_cvd(rgb: (u8, u8, u8), deuteranopia: bool) -> (u8, u8, u8) {
    let (r, g, b) = (
        srgb_to_linear(rgb.0),
        srgb_to_linear(rgb.1),
        srgb_to_linear(rgb.2),
    );
    let (r2, g2, b2) = if deuteranopia {
        (0.625 * r + 0.375 * g, 0.7 * r + 0.3 * g, 0.3 * g + 0.7 * b)
    } else {
        (
            0.567 * r + 0.433 * g,
            0.558 * r + 0.442 * g,
            0.242 * g + 0.758 * b,
        )
    };
    (linear_to_srgb(r2), linear_to_srgb(g2), linear_to_srgb(b2))
}

fn colorblind_safety(palette: &Palette) -> Criterion {
    let mut findings = Vec::new();
    let mut scores = Vec::new();
    for (label, deuteranopia) in [("deuteranopia", true), ("protanopia", false)] {
        for (left, right) in MUST_DISTINGUISH.iter().copied() {
            let a = Oklab::from_rgb(simulate_cvd(palette.rgb(left), deuteranopia));
            let b = Oklab::from_rgb(simulate_cvd(palette.rgb(right), deuteranopia));
            let distance = a.distance(b);
            let score = ratio_score(distance, DISTINCT_TARGET);
            if score < 0.7 {
                findings.push(format!(
                    "{} and {} become indistinguishable under {label} (distance {:.2})",
                    left.key(),
                    right.key(),
                    distance
                ));
            }
            scores.push(score);
        }
    }
    let mean = aggregate(&scores);
    findings.sort();
    findings.dedup();
    Criterion {
        name: "colorblind safety",
        score: as_percent(mean),
        // Advisory rather than critical: red/green semantics are a terminal-wide
        // convention that essentially every established palette follows, so
        // treating it as a hard defect would mark all of them broken. The
        // findings still tell a user exactly which pairs to fix.
        weight: 1.0,
        findings,
        critical: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DARK_BG: (u8, u8, u8) = (18, 18, 18);

    #[test]
    fn oklab_round_trips_within_one_step() {
        for rgb in [
            (0, 0, 0),
            (255, 255, 255),
            (138, 180, 248),
            (255, 193, 7),
            (35, 40, 50),
        ] {
            let back = Oklab::from_rgb(rgb).to_rgb();
            for (a, b) in [(rgb.0, back.0), (rgb.1, back.1), (rgb.2, back.2)] {
                assert!(
                    (a as i16 - b as i16).abs() <= 1,
                    "round trip drifted: {rgb:?} -> {back:?}"
                );
            }
        }
    }

    #[test]
    fn oklab_lightness_orders_colors_as_the_eye_does() {
        let black = Oklab::from_rgb((0, 0, 0)).l;
        let gray = Oklab::from_rgb((128, 128, 128)).l;
        let white = Oklab::from_rgb((255, 255, 255)).l;
        assert!(black < gray && gray < white);
        // Perceptual, not linear: mid-gray should read near the middle.
        assert!((0.4..0.7).contains(&gray), "mid gray lightness was {gray}");
    }

    #[test]
    fn hue_is_stable_for_saturated_primaries() {
        let red = Oklab::from_rgb((255, 0, 0)).hue_degrees();
        let green = Oklab::from_rgb((0, 255, 0)).hue_degrees();
        let blue = Oklab::from_rgb((0, 0, 255)).hue_degrees();
        assert!(hue_delta(red, green) > 60.0);
        assert!(hue_delta(green, blue) > 60.0);
        assert!(hue_delta(red, blue) > 60.0);
    }

    #[test]
    fn default_palette_is_at_least_reasonable() {
        let report = analyze(&Palette::default(), DARK_BG);
        assert!(
            report.score >= 55,
            "default palette scored {} ({:?})",
            report.score,
            report.top_findings(5)
        );
    }

    #[test]
    fn unreadable_palette_is_flagged_by_readability() {
        let mut palette = Palette::default();
        // Near-black text on a dark terminal.
        for role in [Role::UserText, Role::AiText, Role::Accent, Role::Info] {
            palette.set(role, (20, 20, 22));
        }
        let report = analyze(&palette, DARK_BG);
        let readability = report
            .criteria
            .iter()
            .find(|criterion| criterion.name == "readability")
            .expect("readability criterion");
        assert!(
            readability.score < 60,
            "expected low readability, got {}",
            readability.score
        );
        assert!(!readability.findings.is_empty());
        assert!(report.score < 75);
    }

    #[test]
    fn colliding_semantic_roles_are_flagged_by_distinctness() {
        let mut palette = Palette::default();
        palette.set(Role::Success, (200, 60, 60));
        palette.set(Role::Error, (205, 62, 62));
        let report = analyze(&palette, DARK_BG);
        let distinctness = report
            .criteria
            .iter()
            .find(|criterion| criterion.name == "distinctness")
            .expect("distinctness criterion");
        assert!(distinctness.score < 80);
        assert!(
            distinctness
                .findings
                .iter()
                .any(|finding| finding.contains("success") && finding.contains("error")),
            "expected a success/error finding, got {:?}",
            distinctness.findings
        );
    }

    #[test]
    fn red_green_pairs_are_flagged_for_colorblind_users() {
        let mut palette = Palette::default();
        // A pure red/green pair: clearly distinct to trichromats, not to others.
        palette.set(Role::Success, (0, 200, 0));
        palette.set(Role::Error, (200, 120, 0));
        let report = analyze(&palette, DARK_BG);
        let safety = report
            .criteria
            .iter()
            .find(|criterion| criterion.name == "colorblind safety")
            .expect("colorblind criterion");
        assert!(
            !safety.findings.is_empty(),
            "expected a colorblind finding for a red/green pair"
        );
    }

    #[test]
    fn a_deliberately_harmonious_palette_beats_a_random_one() {
        // Analogous, evenly saturated, all readable on dark.
        let mut tuned = Palette::default();
        for (role, rgb) in [
            (Role::User, (120, 170, 245)),
            (Role::Ai, (120, 215, 200)),
            (Role::Accent, (150, 150, 250)),
            (Role::Info, (110, 190, 240)),
            (Role::Success, (120, 210, 160)),
            (Role::Warning, (240, 200, 120)),
            (Role::Error, (245, 130, 140)),
        ] {
            tuned.set(role, rgb);
        }

        // Scattered hues, wildly mixed saturation, some unreadable.
        let mut chaotic = Palette::default();
        for (role, rgb) in [
            (Role::User, (255, 0, 255)),
            (Role::Ai, (60, 60, 62)),
            (Role::Accent, (0, 255, 0)),
            (Role::Info, (128, 90, 20)),
            (Role::Success, (0, 40, 255)),
            (Role::Warning, (30, 30, 35)),
            (Role::Error, (10, 200, 190)),
        ] {
            chaotic.set(role, rgb);
        }

        let tuned_score = analyze(&tuned, DARK_BG).score;
        let chaotic_score = analyze(&chaotic, DARK_BG).score;
        assert!(
            tuned_score > chaotic_score + 10,
            "tuned {tuned_score} should clearly beat chaotic {chaotic_score}"
        );
    }

    #[test]
    fn scores_are_stable_and_bounded() {
        let report = analyze(&Palette::default(), DARK_BG);
        assert!(report.score <= 100);
        for criterion in &report.criteria {
            assert!(criterion.score <= 100, "{} out of range", criterion.name);
        }
        // Determinism: the same palette must always score the same.
        assert_eq!(report, analyze(&Palette::default(), DARK_BG));
    }

    #[test]
    fn grade_labels_track_the_score() {
        let mut report = analyze(&Palette::default(), DARK_BG);
        report.score = 95;
        assert_eq!(report.grade(), "excellent");
        report.score = 10;
        assert_eq!(report.grade(), "broken");
    }

    #[test]
    fn light_background_changes_readability_verdicts() {
        let mut palette = Palette::default();
        // Near-white text: great on dark, unreadable on light.
        palette.set(Role::AiText, (250, 250, 250));
        let dark = analyze(&palette, (18, 18, 18));
        let light = analyze(&palette, (255, 255, 255));
        let score_of = |report: &HarmonyReport| {
            report
                .criteria
                .iter()
                .find(|criterion| criterion.name == "readability")
                .expect("readability")
                .score
        };
        assert!(
            score_of(&dark) > score_of(&light),
            "dark {} should beat light {}",
            score_of(&dark),
            score_of(&light)
        );
    }
}

/// Calibration against real, widely-used terminal palettes.
///
/// A harmony score is only useful if it agrees with human judgement. These
/// tests pin that agreement: palettes that thousands of developers chose on
/// purpose (Solarized, Gruvbox, Dracula, Nord) must all score well, and
/// deliberately hostile palettes must score badly. If a scoring tweak inverts
/// any of these, the metric has drifted away from what people actually mean by
/// "harmonious" and the tweak is wrong.
#[cfg(test)]
pub(crate) mod calibration {
    use super::*;
    use crate::palette::parse_hex;

    const DARK_BG: (u8, u8, u8) = (18, 18, 18);

    fn build(pairs: &[(Role, &str)]) -> Palette {
        let mut palette = Palette::default();
        for (role, hex) in pairs {
            palette.set(*role, parse_hex(hex).expect("test palettes use valid hex"));
        }
        palette
    }

    pub(super) fn solarized_dark() -> Palette {
        build(&[
            (Role::User, "#268bd2"),
            (Role::Ai, "#859900"),
            (Role::Accent, "#6c71c4"),
            (Role::System, "#d33682"),
            (Role::Info, "#2aa198"),
            (Role::Success, "#859900"),
            (Role::Warning, "#b58900"),
            (Role::Error, "#dc322f"),
            (Role::AiText, "#93a1a1"),
            (Role::UserText, "#eee8d5"),
            (Role::Dim, "#586e75"),
            (Role::UserBg, "#073642"),
            (Role::SelectionBg, "#073642"),
        ])
    }

    pub(super) fn gruvbox_dark() -> Palette {
        build(&[
            (Role::User, "#83a598"),
            (Role::Ai, "#b8bb26"),
            (Role::Accent, "#d3869b"),
            (Role::System, "#fe8019"),
            (Role::Info, "#8ec07c"),
            (Role::Success, "#b8bb26"),
            (Role::Warning, "#fabd2f"),
            (Role::Error, "#fb4934"),
            (Role::AiText, "#ebdbb2"),
            (Role::UserText, "#fbf1c7"),
            (Role::Dim, "#665c54"),
            (Role::UserBg, "#3c3836"),
            (Role::SelectionBg, "#504945"),
        ])
    }

    pub(super) fn dracula() -> Palette {
        build(&[
            (Role::User, "#8be9fd"),
            (Role::Ai, "#50fa7b"),
            (Role::Accent, "#bd93f9"),
            (Role::System, "#ff79c6"),
            (Role::Info, "#8be9fd"),
            (Role::Success, "#50fa7b"),
            (Role::Warning, "#f1fa8c"),
            (Role::Error, "#ff5555"),
            (Role::AiText, "#f8f8f2"),
            (Role::UserText, "#f8f8f2"),
            (Role::Dim, "#6272a4"),
            (Role::UserBg, "#282a36"),
            (Role::SelectionBg, "#44475a"),
        ])
    }

    pub(super) fn nord() -> Palette {
        build(&[
            (Role::User, "#81a1c1"),
            (Role::Ai, "#a3be8c"),
            (Role::Accent, "#b48ead"),
            (Role::System, "#d08770"),
            (Role::Info, "#88c0d0"),
            (Role::Success, "#a3be8c"),
            (Role::Warning, "#ebcb8b"),
            (Role::Error, "#bf616a"),
            (Role::AiText, "#eceff4"),
            (Role::UserText, "#e5e9f0"),
            (Role::Dim, "#4c566a"),
            (Role::UserBg, "#3b4252"),
            (Role::SelectionBg, "#434c5e"),
        ])
    }

    /// Every role a barely-different shade of mud: unreadable and indistinct.
    pub(super) fn all_mud() -> Palette {
        build(&[
            (Role::User, "#4a4438"),
            (Role::Ai, "#4b4536"),
            (Role::Accent, "#494339"),
            (Role::System, "#484338"),
            (Role::Info, "#484237"),
            (Role::Success, "#4a4539"),
            (Role::Warning, "#4b4437"),
            (Role::Error, "#494338"),
            (Role::AiText, "#4a4438"),
            (Role::UserText, "#4a4539"),
        ])
    }

    /// Maximum-saturation colors at scattered hues: readable but jarring.
    pub(super) fn neon_chaos() -> Palette {
        build(&[
            (Role::User, "#ff00ff"),
            (Role::Ai, "#00ff00"),
            (Role::Accent, "#ffff00"),
            (Role::System, "#00ffff"),
            (Role::Info, "#ff8000"),
            (Role::Success, "#0000ff"),
            (Role::Warning, "#ff0080"),
            (Role::Error, "#80ff00"),
        ])
    }

    fn score(palette: &Palette) -> u8 {
        analyze(palette, DARK_BG).score
    }

    #[test]
    fn respected_community_palettes_all_score_well() {
        for (name, palette) in [
            ("solarized dark", solarized_dark()),
            ("gruvbox dark", gruvbox_dark()),
            ("dracula", dracula()),
            ("nord", nord()),
        ] {
            let report = analyze(&palette, DARK_BG);
            assert!(
                report.score >= 60,
                "{name} is a widely loved palette but scored {} ({:?})",
                report.score,
                report.top_findings(3)
            );
        }
    }

    #[test]
    fn hostile_palettes_score_far_below_good_ones() {
        let worst_good = [
            score(&solarized_dark()),
            score(&gruvbox_dark()),
            score(&dracula()),
            score(&nord()),
        ]
        .into_iter()
        .min()
        .expect("non-empty");

        for (name, palette) in [("all mud", all_mud()), ("neon chaos", neon_chaos())] {
            let bad = score(&palette);
            assert!(
                bad + 10 <= worst_good,
                "{name} scored {bad}, too close to the worst good palette ({worst_good})"
            );
        }
    }

    #[test]
    fn an_unreadable_palette_is_ranked_below_a_merely_garish_one() {
        // Ordering matters more than absolute numbers: text you cannot read is
        // a worse failure than text that clashes.
        assert!(
            score(&all_mud()) < score(&neon_chaos()),
            "unreadable ({}) should rank below garish ({})",
            score(&all_mud()),
            score(&neon_chaos())
        );
    }

    #[test]
    fn every_role_is_covered_by_at_least_one_criterion() {
        // A role nobody scores is a role users can silently break. Each role
        // must appear in readability (all roles) and, if semantic, in the
        // must-distinguish pairs.
        let mut broken = Palette::default();
        for role in ALL_ROLES.iter().copied() {
            // Make this role maximally wrong and confirm the score notices.
            let mut palette = Palette::default();
            palette.set(
                role,
                if role.is_background() {
                    (255, 0, 255)
                } else {
                    (19, 19, 19)
                },
            );
            let degraded = analyze(&palette, DARK_BG).score;
            let baseline = analyze(&Palette::default(), DARK_BG).score;
            if degraded >= baseline {
                broken.set(role, (0, 0, 0));
            }
        }
        assert!(
            !broken.has_overrides(),
            "some roles can be broken without lowering the score"
        );
    }
}

#[cfg(test)]
mod score_probe {
    use super::*;
    /// Prints the calibration scores documented in
    /// `docs/TUI_COLOR_CONFIGURATION.md`. Run with `--ignored` when updating
    /// that table so the docs cannot drift from the implementation.
    #[test]
    #[ignore = "reporting helper for the docs table"]
    fn print_documented_scores() {
        for (name, palette) in [
            ("Dracula", super::calibration::dracula()),
            ("Solarized Dark", super::calibration::solarized_dark()),
            ("Nord", super::calibration::nord()),
            ("Gruvbox Dark", super::calibration::gruvbox_dark()),
            ("Neon chaos", super::calibration::neon_chaos()),
            ("Unreadable mud", super::calibration::all_mud()),
        ] {
            println!("| {name} | {} |", analyze(&palette, (18, 18, 18)).score);
        }
    }
}
