//! Bench harness: score light-background palette candidates with the harmony
//! analyzer. Run with:
//!   cargo run -p jcode-tui-style --example light_bench

use jcode_tui_style::palette::{ALL_ROLES, Palette, Role, parse_hex, to_hex};
use jcode_tui_style::{HarmonyReport, Oklab, analyze_harmony};

const LIGHT_BG: (u8, u8, u8) = (255, 255, 255);

fn build(pairs: &[(Role, &str)]) -> Palette {
    let mut palette = Palette::default();
    for (role, hex) in pairs {
        palette.set(*role, parse_hex(hex).expect("valid hex"));
    }
    palette
}

/// The current runtime behavior: hue-preserving HSL luminance flip of the
/// dark-native defaults (mirrors theme_mode::flip_luminance).
fn flipped_defaults() -> Palette {
    let mut palette = Palette::default();
    for role in ALL_ROLES.iter().copied() {
        let (r, g, b) = role.default_rgb();
        let (h, s, l) = rgb_to_hsl(r, g, b);
        palette.set(role, hsl_to_rgb(h, s, 1.0 - l));
    }
    palette
}

fn rgb_to_hsl(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    let r = r as f32 / 255.0;
    let g = g as f32 / 255.0;
    let b = b as f32 / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    if (max - min).abs() < f32::EPSILON {
        return (0.0, 0.0, l);
    }
    let d = max - min;
    let s = if l > 0.5 {
        d / (2.0 - max - min)
    } else {
        d / (max + min)
    };
    let h = if (max - r).abs() < f32::EPSILON {
        ((g - b) / d).rem_euclid(6.0)
    } else if (max - g).abs() < f32::EPSILON {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    } * 60.0;
    (h, s, l)
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (u8, u8, u8) {
    let l = l.clamp(0.0, 1.0);
    if s <= 0.0 {
        let v = (l * 255.0).round() as u8;
        return (v, v, v);
    }
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = (h.rem_euclid(360.0)) / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r1, g1, b1) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    let to_u8 = |v: f32| ((v + m).clamp(0.0, 1.0) * 255.0).round() as u8;
    (to_u8(r1), to_u8(g1), to_u8(b1))
}

/// Hand-tuned light palette candidate.
///
/// Designed in Oklab: chromatic roles sit in a 0.11-0.18 chroma band with
/// lightness 0.40-0.63 (delta-L >= 0.37 against white). CVD-confusable pairs
/// (warning/error, system/queued, success/error) are separated by lightness,
/// not hue, since hue is what deuteranopia/protanopia destroy.
fn hand_tuned() -> Palette {
    build(&[
        (Role::User, "#1467c2"),
        (Role::Ai, "#38853e"),
        (Role::Tool, "#6e7781"),
        (Role::FileLink, "#0256a9"),
        (Role::Dim, "#8c959f"),
        (Role::Accent, "#7444b4"),
        (Role::System, "#c2418f"),
        (Role::Queued, "#8a6404"),
        (Role::Asap, "#0e7d9e"),
        (Role::Pending, "#6e7781"),
        (Role::UserText, "#1f2328"),
        (Role::UserBg, "#eef2f8"),
        (Role::AiText, "#24292f"),
        (Role::HeaderIcon, "#0c758d"),
        (Role::HeaderName, "#2d496d"),
        (Role::HeaderSession, "#1f2328"),
        (Role::Success, "#38853e"),
        (Role::Warning, "#b67c0d"),
        (Role::Error, "#a91228"),
        (Role::Info, "#1577c8"),
        (Role::Border, "#8c959f"),
        (Role::SelectionBg, "#d7e3f4"),
    ])
}

fn report(name: &str, palette: &Palette) -> HarmonyReport {
    let report = analyze_harmony(palette, LIGHT_BG);
    println!(
        "== {name}: score {} ({}), scheme {}",
        report.score,
        report.grade(),
        report.scheme
    );
    for criterion in &report.criteria {
        println!(
            "   {:>20}: {:>3}{}",
            criterion.name,
            criterion.score,
            if criterion.critical { " *" } else { "" }
        );
    }
    for finding in report.top_findings(8) {
        println!("   - {finding}");
    }
    report
}

/// Objective for the tuner: overall score with the summed criterion scores as
/// a tiebreaker, so plateaus in the rounded overall score still make progress.
fn objective(palette: &Palette) -> (u8, u32) {
    let report = analyze_harmony(palette, LIGHT_BG);
    let detail: u32 = report
        .criteria
        .iter()
        .map(|criterion| criterion.score as u32)
        .sum();
    (report.score, detail)
}

/// Roles whose hue is a convention users rely on; the tuner may polish
/// lightness/chroma freely but only nudge hue inside +/- this many degrees.
const HUE_SLACK: f32 = 18.0;

/// Deterministic coordinate descent over (L, C, hue) per role in Oklab.
///
/// This is "hand-tuned, then let the metric finish the job": the starting
/// palette fixes each role's identity (hue family, rough emphasis) and the
/// descent only polishes within those bounds, so the result stays a designed
/// palette rather than whatever the scorer happens to like.
fn tune(start: &Palette) -> Palette {
    let mut best = *start;
    let mut best_score = objective(&best);
    // Fixed role order and step schedule keep this fully deterministic.
    let steps = [0.06f32, 0.03, 0.015, 0.008];
    for &step in &steps {
        let mut improved = true;
        while improved {
            improved = false;
            for role in ALL_ROLES.iter().copied() {
                if matches!(role, Role::UserText | Role::AiText | Role::HeaderSession) {
                    // Body text stays near-neutral dark by design.
                    continue;
                }
                let anchor = Oklab::from_rgb(start.rgb(role));
                let current = Oklab::from_rgb(best.rgb(role));
                let (l, c, h) = (
                    current.l,
                    current.chroma(),
                    current.hue_degrees().to_radians(),
                );
                let mut candidates = Vec::new();
                for (dl, dc, dh) in [
                    (step, 0.0, 0.0),
                    (-step, 0.0, 0.0),
                    (0.0, step * 0.5, 0.0),
                    (0.0, -step * 0.5, 0.0),
                    (0.0, 0.0, step * 60.0),
                    (0.0, 0.0, -step * 60.0),
                ] {
                    let nl = (l + dl).clamp(0.25, 0.97);
                    let nc = (c + dc).clamp(0.0, 0.22);
                    let nh = h + dh.to_radians();
                    let lab = Oklab {
                        l: nl,
                        a: nc * nh.cos(),
                        b: nc * nh.sin(),
                    };
                    // Respect hue identity for chromatic roles.
                    if anchor.chroma() > 0.04 && lab.chroma() > 0.04 {
                        let delta = (lab.hue_degrees() - anchor.hue_degrees()).rem_euclid(360.0);
                        let delta = delta.min(360.0 - delta);
                        if delta > HUE_SLACK {
                            continue;
                        }
                    }
                    candidates.push(lab.to_rgb());
                }
                for rgb in candidates {
                    let mut trial = best;
                    trial.set(role, rgb);
                    let score = objective(&trial);
                    if score > best_score {
                        best = trial;
                        best_score = score;
                        improved = true;
                    }
                }
            }
        }
    }
    best
}

fn main() {
    let derived = flipped_defaults();
    let generated =
        jcode_tui_style::harmony::generate_from_seed(Role::User.default_rgb(), LIGHT_BG);
    let tuned = tune(&hand_tuned());

    report("derived (runtime flip of dark defaults)", &derived);
    println!();
    report("generated (generate_from_seed, jcode blue)", &generated);
    println!();
    report("hand-tuned start", &hand_tuned());
    println!();
    report("hand-tuned + metric descent", &tuned);

    println!("\nfinal palette hex values:");
    for role in ALL_ROLES.iter().copied() {
        println!("  {:>15}: {}", role.key(), to_hex(tuned.rgb(role)));
    }
}
