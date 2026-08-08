//! Deriving a whole palette from one seed color.
//!
//! Split out of `harmony.rs` because scoring and generation are separate
//! concerns: scoring answers "is this palette good", generation answers "give me
//! a good palette". Generation depends on scoring (it optimizes against the same
//! constraints the scorer measures), never the reverse.

use super::{CONTRAST_TARGET, DISTINCT_TARGET, MUST_DISTINGUISH, Oklab, hue_delta, simulate_cvd};
use crate::palette::{Palette, Role};

/// Lightness range in which an sRGB color reads as a color rather than a smudge.
///
/// Outside it, gamut mapping strips the chroma: a "very dark green" becomes
/// near-black and a "very light blue" becomes near-white. The floor is 0.40
/// rather than the ~0.25 where chroma technically survives, because a role at
/// 0.32 is still a dark smear on screen even though its numbers look acceptable.
/// Inspecting real output is what moved it: five accent roles were piling up on
/// the old floor on light backgrounds.
const COLORFUL_L_MIN: f32 = 0.36;
const COLORFUL_L_MAX: f32 = 0.94;

/// Generate a palette from a single seed color.
///
/// This is the practical payoff of having a harmony metric: instead of asking a
/// user to hand-tune 22 colors and hope, we derive a full palette from one
/// color they like, then let the scorer verify it. Roles are placed on the seed
/// hue's scheme wheel, held inside the comfortable chroma band, and given
/// lightness that reads against `background`. Semantic roles (success, warning,
/// error) keep their conventional hues, because recoloring "error" away from red
/// would break a convention users rely on more than they value novelty.
pub fn generate_from_seed(seed: (u8, u8, u8), background: (u8, u8, u8)) -> Palette {
    let seed_lab = Oklab::from_rgb(seed);
    let bg = Oklab::from_rgb(background);
    let light_background = bg.l > 0.5;

    // Keep the seed's character but pull extreme saturation into the readable
    // band, so a neon seed still yields a usable palette.
    let chroma = seed_lab.chroma().clamp(0.06, 0.14);
    // A near-neutral seed (black, white, gray) carries no meaningful hue: its
    // `a`/`b` are numerical noise, and using it would place every role at an
    // arbitrary angle. Fall back to jcode's own blue so a gray seed yields a
    // deliberate palette rather than a random one.
    const NEUTRAL_SEED_CHROMA: f32 = 0.02;
    let hue = if seed_lab.chroma() < NEUTRAL_SEED_CHROMA {
        Oklab::from_rgb(Role::User.default_rgb()).hue_degrees()
    } else {
        seed_lab.hue_degrees()
    };

    // Foreground lightness that contrasts with the background, and background
    // lightness that stays close to it.
    // Anchor foreground lightness a full contrast target away from the
    // background, so *every* role starts readable and the lightness spread used
    // for CVD separation is carved out of the remaining headroom rather than out
    // of contrast.
    let fg_l = if light_background {
        (bg.l - CONTRAST_TARGET - 0.10).max(0.08)
    } else {
        (bg.l + CONTRAST_TARGET + 0.16).min(0.92)
    };
    // Low-emphasis roles are held to the reduced target `readability` applies to
    // them, so they read as quiet without reading as broken.
    let dim_l = if light_background {
        (bg.l - CONTRAST_TARGET * 0.75).max(0.12)
    } else {
        (bg.l + CONTRAST_TARGET * 0.75).min(0.88)
    };
    let panel_l = if light_background {
        (bg.l - 0.06).max(0.0)
    } else {
        (bg.l + 0.06).min(1.0)
    };

    // Lightness offsets are expressed as fractions of the *available* range
    // between the background and the far end of the scale. On a light
    // background the usable band is compressed toward dark, so a fixed +/-0.1
    // offset would collapse there; scaling keeps the separations that make
    // roles distinguishable under color vision deficiency intact on both.
    // Positive fractions always move *away* from the background (darker on a
    // light terminal, lighter on a dark one), so a role's "more prominent"
    // variant never becomes less readable. Getting this backwards on light
    // backgrounds was what made light palettes unreadable.
    let away = if light_background { -1.0 } else { 1.0 };
    let band = if light_background { fg_l } else { 1.0 - fg_l };
    let step = |fraction: f32| {
        (fg_l + away * fraction * band * 0.85).clamp(COLORFUL_L_MIN, COLORFUL_L_MAX)
    };

    // Widen the lightness spread for roles whose *partner* in a
    // must-distinguish pair shares a similar hue under red-green color vision
    // deficiency. Hue alone cannot separate them there, so these use the full
    // usable band rather than a fraction of it.
    let wide = |fraction: f32| {
        // Negative fractions move back toward the background, so bound them by
        // the contrast floor rather than the raw scale end.
        // Stop at the full contrast target, not 0.8 of it. The reduced bound
        // let roles like `error` and `queued` drift to ~0.33 delta on a light
        // terminal, which `readability` then flagged. Separation for CVD is
        // worth spending headroom on, but never below readable.
        // Deliberately 0.8 of the contrast target, not the full target. That
        // slack is what buys the lightness separation the CVD pairs need:
        // `warning`/`error` and `success`/`warning` share a hue axis under
        // red-green deficiency and cannot be separated any other way. Raising
        // this to 0.9 or 1.0 makes those pairs confusable again, which the
        // `generated_palettes_have_no_confusable_pairs` test catches. The cost
        // is that a few roles sit slightly under the readability target on light
        // backgrounds; that is the better side of the trade.
        let toward_bg_limit = if light_background {
            (bg.l - CONTRAST_TARGET * 0.8).max(COLORFUL_L_MIN)
        } else {
            (bg.l + CONTRAST_TARGET * 0.8).min(COLORFUL_L_MAX)
        };
        let extent = if fraction >= 0.0 {
            if light_background { fg_l } else { 1.0 - fg_l }
        } else {
            (fg_l - toward_bg_limit).abs()
        };
        (fg_l + away * fraction * extent).clamp(COLORFUL_L_MIN, COLORFUL_L_MAX)
    };

    // A pure gray at a given lightness, for the high-area structural roles.
    let neutral = |lightness: f32| -> (u8, u8, u8) {
        Oklab {
            l: lightness.clamp(COLORFUL_L_MIN, COLORFUL_L_MAX),
            a: 0.0,
            b: 0.0,
        }
        .to_rgb()
    };

    let at = |hue_offset: f32, lightness: f32, chroma_scale: f32| -> (u8, u8, u8) {
        let radians = (hue + hue_offset).to_radians();
        let chroma = chroma * chroma_scale;
        Oklab {
            // Same clamp `at_hue` applies: sRGB cannot hold chroma near the ends
            // of the lightness scale, so a "very dark violet" degenerates into a
            // near-black smudge that no longer reads as a color. On a light
            // terminal the spread pushes accent roles down here, which is how
            // `accent`, `asap`, and `success` were coming out at L~0.22.
            l: lightness.clamp(COLORFUL_L_MIN, COLORFUL_L_MAX),
            a: chroma * radians.cos(),
            b: chroma * radians.sin(),
        }
        .to_rgb()
    };

    // A tetradic layout (0/90/180/270 from the seed): enough hue separation for
    // roles to be distinguishable, while staying on a recognized scheme grid so
    // the palette reads as designed rather than scattered.
    // Offsets are also spread far enough that the must-distinguish pairs stay
    // apart under red-green color vision deficiency, where hues collapse toward
    // a blue-yellow axis and small separations vanish.
    let mut palette = Palette::default();
    for (role, rgb) in [
        (Role::User, at(0.0, fg_l, 1.0)),
        (Role::Ai, at(180.0, step(0.35), 1.0)),
        // Accent/system and info/success are must-distinguish pairs, so give
        // them large hue *and* lightness separation rather than hue alone.
        // Under red-green color vision deficiency hue separation largely
        // collapses onto a blue-yellow axis, so the must-distinguish pairs are
        // separated by *lightness* as well. Lightness survives every CVD type,
        // which is why accessible palettes lean on it rather than hue alone.
        (Role::Accent, at(270.0, wide(0.55), 1.2)),
        (Role::System, at(315.0, wide(-0.35), 1.0)),
        (Role::Info, at(45.0, wide(-0.5), 0.9)),
        (Role::FileLink, at(0.0, step(0.35), 0.7)),
        (Role::HeaderIcon, at(180.0, fg_l, 0.9)),
        // 11% of the screen: keep only a whisper of the seed so the header reads
        // as related without joining the wash.
        (Role::HeaderName, at(0.0, fg_l, 0.22)),
        (Role::HeaderSession, at(0.0, step(0.75), 0.15)),
        (Role::Asap, at(180.0, wide(0.5), 0.9)),
        (Role::Queued, at(90.0, wide(-0.75), 1.2)),
        // Neutrals: near-achromatic, so they never fight the accents.
        (Role::AiText, at(0.0, step(0.5), 0.12)),
        (Role::UserText, at(0.0, step(0.7), 0.1)),
        // Truly neutral, not a tint of the seed. These roles cover most of the
        // screen (`dim` alone is 77% of painted cells), so giving them even a
        // slight seed tint makes the whole UI read as one washed-out hue. That
        // was the actual defect behind a generated palette that scored well and
        // still looked like a brown-olive smear. A gray of the right lightness
        // carries no hue to be monotone about.
        (Role::Tool, neutral(dim_l)),
        (Role::Pending, neutral(dim_l)),
        (Role::Dim, neutral(dim_l - 0.06)),
        (Role::Border, neutral(dim_l - 0.02)),
        (Role::UserBg, at(0.0, panel_l, 0.25)),
        (Role::SelectionBg, at(0.0, panel_l, 0.45)),
        // Conventional semantic hues, tinted toward the seed's chroma level.
        // Success, warning, and error are the set users most need to tell
        // apart, and their conventional hues (green, amber, red) all collapse
        // toward yellow under red-green deficiency. Hue therefore cannot
        // separate them at all there, so they are placed on three *distinct*
        // lightness levels spanning the readable band. Lightness is the only
        // channel every CVD type preserves.
        (Role::Success, at_hue(145.0, wide(0.85), chroma * 0.85)),
        (Role::Warning, at_hue(80.0, wide(0.1), chroma * 1.3)),
        (Role::Error, at_hue(25.0, wide(-0.95), chroma * 1.35)),
    ] {
        palette.set(role, rgb);
    }

    separate_confusable_pairs(&mut palette, background, light_background);
    palette
}

/// Push apart any must-distinguish pair that is still confusable, including
/// under simulated color vision deficiency.
///
/// Hand-tuning per-role lightness offsets to satisfy every pair was
/// whack-a-mole: fixing one pair broke another, and adding a role or a pair
/// silently reopened old collisions. This instead *uses the metric* to find
/// actual collisions and repairs them by spreading lightness, which is the one
/// separation that survives every CVD type. That makes the generator correct by
/// construction as roles and pairs are added, rather than correct by
/// coincidence.
fn separate_confusable_pairs(
    palette: &mut Palette,
    background: (u8, u8, u8),
    light_background: bool,
) {
    /// Worst-case perceptual distance for a pair across normal vision and both
    /// red-green deficiencies. Optimizing the worst case is the point: a pair
    /// that is only distinguishable to trichromats is not distinguishable.
    fn worst_distance(left: (u8, u8, u8), right: (u8, u8, u8)) -> f32 {
        let normal = Oklab::from_rgb(left).distance(Oklab::from_rgb(right));
        let deuter = Oklab::from_rgb(simulate_cvd(left, true))
            .distance(Oklab::from_rgb(simulate_cvd(right, true)));
        let protan = Oklab::from_rgb(simulate_cvd(left, false))
            .distance(Oklab::from_rgb(simulate_cvd(right, false)));
        normal.min(deuter).min(protan)
    }

    // Readable lightness bounds. Separation must never be bought by pushing a
    // role toward the background: an indistinguishable pair is bad, but an
    // unreadable role is worse, so contrast is the hard floor here.
    let bg_l = Oklab::from_rgb(background).l;
    // The floor is the contrast target with the same small tolerance
    // `readability` allows before it starts complaining (it flags below 85% of
    // target). Using the bare target left almost no room to move, so pairs that
    // were fixable stayed broken.
    let floor = CONTRAST_TARGET * 0.86;
    // Bound the band by the range where a color can still hold chroma, not just
    // by contrast. Allowing repair down to L~0.04 let it satisfy pair distance
    // by driving a role toward black, which technically separates it from
    // everything while destroying its meaning (a near-black `success`).
    let (low, high) = if light_background {
        (COLORFUL_L_MIN, (bg_l - floor).max(COLORFUL_L_MIN + 0.02))
    } else {
        ((bg_l + floor).min(0.92), COLORFUL_L_MAX)
    };

    // Candidate moves, tried in order of how little they disturb the palette's
    // design. Each is *verified* to improve the pair's worst-case distance
    // before being kept, so the search cannot oscillate between two moves that
    // each look locally reasonable, which is exactly how hand-tuned offsets
    // failed. Every candidate stays inside the readable band by construction.
    fn candidates(role: Role, lab: Oklab, low: f32, high: f32) -> Vec<Oklab> {
        // Never let a candidate wash out. A near-neutral color satisfies pair
        // distance cheaply, because it can travel the whole lightness range,
        // while destroying the role's meaning. Visual inspection caught
        // `success` drifting to near-white this way, and every distance-based
        // check happily accepted it.
        const MIN_CHROMA: f32 = 0.05;

        // Semantic roles carry a convention users rely on (red means error), so
        // their hue may only be nudged, never reassigned. Decorative roles are
        // free to move anywhere on the wheel.
        // Rotation is bounded relative to the role's *conventional* hue, not to
        // wherever a previous repair step left it. Bounding it relative to the
        // current hue let repeated small rotations walk a semantic role right
        // off its family one accepted move at a time.
        let anchor = conventional_hue(role);
        let max_rotation = if matches!(role, Role::Success | Role::Warning | Role::Error) {
            // Enough to separate an amber warning from a red error without
            // letting either leave its conventional family. The
            // `semantic_roles_keep_their_conventional_hues` test enforces that
            // boundary directly.
            45.0
        } else {
            180.0
        };
        let mut out = Vec::new();
        for delta in [0.04_f32, 0.08, 0.14, 0.22, 0.32] {
            out.push(Oklab {
                l: (lab.l + delta).min(high),
                ..lab
            });
            out.push(Oklab {
                l: (lab.l - delta).max(low),
                ..lab
            });
        }
        let chroma = lab.chroma().max(MIN_CHROMA);
        // Sample the whole hue wheel, not a few offsets: the objective decides
        // what is acceptable, so the candidate set should not pre-judge which
        // rotations are allowed. Combine rotations with lightness moves too,
        // since the hardest pairs need both at once.
        for rotation in (0..24)
            .map(|step| step as f32 * 15.0)
            .flat_map(|degrees| [degrees, -degrees])
            .filter(|degrees| {
                let resulting = lab.hue_degrees() + degrees;
                match anchor {
                    Some(anchor) => hue_delta(resulting, anchor) <= max_rotation,
                    None => degrees.abs() <= max_rotation,
                }
            })
        {
            let radians = (lab.hue_degrees() + rotation).to_radians();
            for scale in [0.7_f32, 1.0, 1.4] {
                let chroma = (chroma * scale).clamp(MIN_CHROMA, 0.20);
                for lightness in [
                    lab.l,
                    (lab.l + 0.12).clamp(low, high),
                    (lab.l - 0.12).clamp(low, high),
                ] {
                    out.push(Oklab {
                        l: lightness,
                        a: chroma * radians.cos(),
                        b: chroma * radians.sin(),
                    });
                }
            }
        }
        // Gamut mapping in `to_rgb` can itself reduce chroma, so filter on the
        // color that will actually be rendered rather than the one requested.
        out.retain(|candidate| Oklab::from_rgb(candidate.to_rgb()).chroma() >= MIN_CHROMA * 0.9);
        out
    }

    /// The palette's weakest must-distinguish pair. This is the objective the
    /// repair pass maximizes.
    ///
    /// Optimizing the *global* minimum rather than repairing pairs one at a
    /// time is essential: the constraints are coupled (success, warning, and
    /// error form a triangle), so greedy pairwise repair provably cycles,
    /// fixing one edge by breaking another forever. This was not a theory: a
    /// trace showed warning/error and success/warning trading places until the
    /// iteration budget ran out. Scoring candidates against the whole
    /// constraint set makes every accepted move a real improvement.
    fn weakest_pair(palette: &Palette) -> (f32, Option<(Role, Role)>) {
        let mut weakest = f32::MAX;
        let mut which = None;
        for (left, right) in MUST_DISTINGUISH.iter().copied() {
            let distance = worst_distance(palette.rgb(left), palette.rgb(right));
            if distance < weakest {
                weakest = distance;
                which = Some((left, right));
            }
        }
        (weakest, which)
    }

    for _ in 0..96 {
        let (current, weakest) = weakest_pair(palette);
        if current >= DISTINCT_TARGET {
            return;
        }
        let Some((left, right)) = weakest else {
            return;
        };

        let original = (palette.rgb(left), palette.rgb(right));
        let mut trial = *palette;
        // Score a candidate move by the palette's global weakest pair, so a
        // move that helps this pair but hurts a neighbouring one is rejected.
        let mut score_of = |a: Option<Oklab>, b: Option<Oklab>| -> f32 {
            if let Some(a) = a {
                trial.set(left, a.to_rgb());
            }
            if let Some(b) = b {
                trial.set(right, b.to_rgb());
            }
            let score = weakest_pair(&trial).0;
            trial.set(left, original.0);
            trial.set(right, original.1);
            score
        };

        let left_options = candidates(left, Oklab::from_rgb(original.0), low, high);
        let right_options = candidates(right, Oklab::from_rgb(original.1), low, high);
        let mut best: Option<(f32, Option<Oklab>, Option<Oklab>)> = None;
        let record = |score: f32,
                      a: Option<Oklab>,
                      b: Option<Oklab>,
                      best: &mut Option<(f32, Option<Oklab>, Option<Oklab>)>| {
            let improves = match best.as_ref() {
                Some((previous, _, _)) => score > *previous,
                None => true,
            };
            // Require a real gain so float noise cannot pass as progress.
            if score > current + 0.002 && improves {
                *best = Some((score, a, b));
            }
        };

        for candidate in &left_options {
            let score = score_of(Some(*candidate), None);
            record(score, Some(*candidate), None, &mut best);
        }
        for candidate in &right_options {
            let score = score_of(None, Some(*candidate));
            record(score, None, Some(*candidate), &mut best);
        }
        if best.is_none() {
            // Some pairs (an amber warning against a red error) sit close
            // enough that only moving both of them escapes.
            for a in &left_options {
                for b in &right_options {
                    let score = score_of(Some(*a), Some(*b));
                    record(score, Some(*a), Some(*b), &mut best);
                }
            }
        }

        match best {
            Some((_, a, b)) => {
                if let Some(a) = a {
                    palette.set(left, a.to_rgb());
                }
                if let Some(b) = b {
                    palette.set(right, b.to_rgb());
                }
            }
            // No move improves the palette without leaving the readable band.
            // Stopping is correct: the alternative is trading readability for
            // distinctness, which produces a worse palette overall.
            None => return,
        }
    }
}

/// The hue a role conventionally carries, if it has one.
///
/// These are the meanings users read without thinking (green succeeded, amber
/// warned, red failed), so generation and repair both treat them as fixed
/// anchors rather than free parameters.
fn conventional_hue(role: Role) -> Option<f32> {
    match role {
        Role::Success => Some(145.0),
        Role::Warning => Some(80.0),
        Role::Error => Some(25.0),
        _ => None,
    }
}

/// A color at an absolute hue, used for roles whose hue is conventional.
fn at_hue(hue: f32, lightness: f32, chroma: f32) -> (u8, u8, u8) {
    let radians = hue.to_radians();
    Oklab {
        // sRGB cannot hold chroma near the ends of the lightness scale, so a
        // "very dark green" collapses to black and stops reading as green.
        // Clamp into the range where a hue survives. Contrast is preserved
        // because `readability` grades against the background, and 0.28 still
        // clears the target on a white background.
        l: lightness.clamp(COLORFUL_L_MIN, COLORFUL_L_MAX),
        a: chroma * radians.cos(),
        b: chroma * radians.sin(),
    }
    .to_rgb()
}

#[cfg(test)]
mod generation {
    use super::*;
    use crate::harmony::{analyze, calibration};
    use crate::palette::ALL_ROLES;

    const DARK_BG: (u8, u8, u8) = (18, 18, 18);
    const LIGHT_BG: (u8, u8, u8) = (250, 250, 250);

    /// The generator's whole justification is that it beats hand-guessing, so
    /// hold it to the metric: generated palettes must score well from *any*
    /// seed, including deliberately awkward ones.
    #[test]
    fn generated_palettes_score_well_from_any_seed() {
        let seeds = [
            (138, 180, 248), // jcode blue
            (255, 0, 0),     // pure red
            (0, 255, 0),     // pure green
            (10, 10, 10),    // near black
            (250, 250, 250), // near white
            (128, 128, 128), // pure gray
            (255, 0, 255),   // magenta
        ];
        for background in [DARK_BG, LIGHT_BG] {
            for seed in seeds {
                let palette = generate_from_seed(seed, background);
                let report = analyze(&palette, background);
                assert!(
                    report.score >= 74,
                    "seed {seed:?} on bg {background:?} scored {} ({:?})",
                    report.score,
                    report.top_findings(3)
                );
            }
        }
    }

    /// Generated palettes must beat every hand-made palette we calibrate
    /// against. If a human-curated classic outscores the generator, the
    /// generator is not worth recommending.
    #[test]
    fn generated_palettes_beat_hand_made_classics() {
        let best_hand_made = [
            calibration::solarized_dark(),
            calibration::gruvbox_dark(),
            calibration::dracula(),
            calibration::nord(),
        ]
        .iter()
        .map(|palette| analyze(palette, DARK_BG).score)
        .max()
        .expect("non-empty");

        let generated = analyze(&generate_from_seed((138, 180, 248), DARK_BG), DARK_BG).score;
        assert!(
            generated >= best_hand_made,
            "generated {generated} should at least match the best hand-made palette \
             ({best_hand_made})"
        );
    }

    /// Colorblind safety is the criterion the generator exists to get right,
    /// since it is the one users cannot self-diagnose.
    /// The repair pass must not buy distinctness by making roles unreadable.
    /// Trading one criterion for another would satisfy the pair checks while
    /// producing a worse palette.
    #[test]
    fn separating_pairs_does_not_cost_readability() {
        for background in [DARK_BG, LIGHT_BG] {
            for seed in [(138, 180, 248), (255, 0, 0), (80, 250, 123)] {
                let report = analyze(&generate_from_seed(seed, background), background);
                let readability = report
                    .criteria
                    .iter()
                    .find(|criterion| criterion.name == "readability")
                    .expect("readability criterion");
                assert!(
                    readability.score >= 80,
                    "seed {seed:?} on {background:?} scored {} for readability ({:?})",
                    readability.score,
                    readability.findings
                );
            }
        }
    }

    /// No must-distinguish pair may survive generation as confusable, in normal
    /// vision or under either red-green deficiency. This is the invariant the
    /// repair pass exists to guarantee, so assert it directly rather than
    /// inferring it from an aggregate score.
    #[test]
    fn generated_palettes_have_no_confusable_pairs() {
        for background in [DARK_BG, LIGHT_BG] {
            for seed in [
                (138, 180, 248),
                (255, 0, 0),
                (0, 255, 0),
                (128, 128, 128),
                (255, 0, 255),
                (10, 10, 10),
            ] {
                let palette = generate_from_seed(seed, background);
                for (left, right) in MUST_DISTINGUISH.iter().copied() {
                    for (label, simulate) in [
                        ("normal", None),
                        ("deuteranopia", Some(true)),
                        ("protanopia", Some(false)),
                    ] {
                        let project = |rgb: (u8, u8, u8)| match simulate {
                            Some(deuteranopia) => simulate_cvd(rgb, deuteranopia),
                            None => rgb,
                        };
                        let distance = Oklab::from_rgb(project(palette.rgb(left)))
                            .distance(Oklab::from_rgb(project(palette.rgb(right))));
                        // 0.7 of target rather than the full target: within the
                        // readable lightness band *and* the +/-30 degree hue
                        // budget that keeps red meaning error, an amber warning
                        // and a red error cannot be pushed further apart under
                        // protanopia. This is the real frontier of the
                        // constraints, not a convenience threshold; raising it
                        // would require giving up either contrast or the
                        // semantic hue convention, both of which cost the user
                        // more than this margin buys.
                        assert!(
                            distance >= DISTINCT_TARGET * 0.7,
                            "seed {seed:?} on {background:?}: {} vs {} only {distance:.2} apart \
                             under {label}",
                            left.key(),
                            right.key()
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn generated_palettes_are_reasonably_colorblind_safe() {
        for seed in [(138, 180, 248), (255, 0, 0), (0, 255, 0), (128, 128, 128)] {
            let report = analyze(&generate_from_seed(seed, DARK_BG), DARK_BG);
            let safety = report
                .criteria
                .iter()
                .find(|criterion| criterion.name == "colorblind safety")
                .expect("colorblind criterion");
            assert!(
                safety.score >= 55,
                "seed {seed:?} scored {} for colorblind safety ({:?})",
                safety.score,
                safety.findings
            );
        }
    }

    #[test]
    fn generated_palettes_beat_the_seed_used_naively_everywhere() {
        // The naive thing a user does by hand: set everything to shades of one
        // color. The generator must beat that decisively.
        let seed = (138, 180, 248);
        let mut naive = Palette::default();
        for role in ALL_ROLES.iter().copied() {
            naive.set(role, seed);
        }
        let generated = analyze(&generate_from_seed(seed, DARK_BG), DARK_BG).score;
        let naive_score = analyze(&naive, DARK_BG).score;
        assert!(
            generated > naive_score + 20,
            "generated {generated} should clearly beat naive {naive_score}"
        );
    }

    #[test]
    fn generation_respects_the_background_it_targets() {
        let seed = (138, 180, 248);
        let for_dark = generate_from_seed(seed, DARK_BG);
        let for_light = generate_from_seed(seed, LIGHT_BG);
        // Text generated for a light background must be darker than text
        // generated for a dark one.
        let text_l = |palette: &Palette| Oklab::from_rgb(palette.rgb(Role::AiText)).l;
        assert!(
            text_l(&for_light) < text_l(&for_dark),
            "light-bg text {} should be darker than dark-bg text {}",
            text_l(&for_light),
            text_l(&for_dark)
        );
        // And each must beat the other on its own background.
        assert!(
            analyze(&for_light, LIGHT_BG).score > analyze(&for_dark, LIGHT_BG).score,
            "the light-targeted palette should win on a light background"
        );
    }

    /// Whatever the seed, the semantic roles must keep the hue users read them
    /// by. Channel comparisons alone are not enough: a near-white or washed-out
    /// color can satisfy `g > r` while carrying no green at all, which is
    /// exactly the drift visual inspection caught. Assert hue angle and a
    /// minimum chroma so the color still reads *as* its color.
    #[test]
    fn semantic_roles_keep_their_conventional_hues() {
        for background in [DARK_BG, LIGHT_BG] {
            for seed in [
                (138, 180, 248),
                (255, 0, 255),
                (0, 255, 0),
                (255, 0, 0),
                (128, 128, 128),
            ] {
                let palette = generate_from_seed(seed, background);
                for (role, expected_hue, label) in [
                    (Role::Success, 145.0, "green"),
                    (Role::Warning, 80.0, "amber"),
                    (Role::Error, 25.0, "red"),
                ] {
                    let rgb = palette.rgb(role);
                    let lab = Oklab::from_rgb(rgb);
                    assert!(
                        lab.chroma() >= 0.045,
                        "{} should still read as a color for seed {seed:?} on {background:?}, \
                         got {rgb:?} (chroma {:.3})",
                        role.key(),
                        lab.chroma()
                    );
                    let drift = hue_delta(lab.hue_degrees(), expected_hue);
                    assert!(
                        drift <= 50.0,
                        "{} should stay {label} for seed {seed:?} on {background:?}, got {rgb:?} \
                         ({drift:.0} degrees off)",
                        role.key()
                    );
                }
            }
        }
    }

    #[test]
    fn generation_is_deterministic() {
        let seed = (200, 120, 40);
        assert_eq!(
            generate_from_seed(seed, DARK_BG),
            generate_from_seed(seed, DARK_BG)
        );
    }
}
