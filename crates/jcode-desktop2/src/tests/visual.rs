//! Pixel-level visual invariants: render every state-space node offscreen and
//! assert what only real output can prove (regions stay clear, text is legible,
//! the caret and selection land where they should). These need a GPU, so they
//! are `#[ignore]`d; run with `cargo test -p jcode-desktop2 -- --ignored`.

use crate::{Model, build_scene, layout::Frame, states, text::TextSystem};
use vello::Scene;

const WIDTH: u32 = 1400;
const HEIGHT: u32 = 900;
const SCALE: f64 = 1.75;

struct Rendered {
    pixels: Vec<u8>,
    width: u32,
    height: u32,
    frame: Frame,
}

impl Rendered {
    fn new(model: &Model) -> Option<Self> {
        Self::at(model, WIDTH, HEIGHT, SCALE)
    }

    /// Render one model at an explicit surface size and scale factor.
    fn at(model: &Model, width: u32, height: u32, scale: f64) -> Option<Self> {
        let mut text = TextSystem::default();
        let mut scene = Scene::new();
        build_scene(&mut scene, &mut text, model, (width, height), scale);
        let pixels = crate::capture::capture_scene_to_rgba(&scene, width, height).ok()?;
        Some(Self {
            pixels,
            width,
            height,
            // Must be the same frame `build_scene` used: sized from the
            // model's *wrapped* row count, via the shared helper so the two
            // can never disagree.
            frame: crate::App::frame_for_model((width, height), scale, model),
        })
    }

    /// Height in physical pixels of the inked rows within a logical rect.
    /// Used to verify text is rasterized at physical size (HiDPI), not
    /// laid out at 1x and left tiny on a scaled display.
    fn ink_rows(&self, x0: f64, y0: f64, x1: f64, y1: f64) -> u32 {
        let s = self.frame.scale;
        let cx = |v: f64| (v * s).round().clamp(0.0, f64::from(self.width - 1)) as u32;
        let cy = |v: f64| (v * s).round().clamp(0.0, f64::from(self.height - 1)) as u32;
        let (px0, px1) = (cx(x0), cx(x1));
        let mut rows = 0;
        for y in cy(y0)..=cy(y1) {
            if (px0..=px1).any(|x| self.luma(x, y) < 0.6) {
                rows += 1;
            }
        }
        rows
    }

    /// Luminance at a physical pixel, 0.0 (black) to 1.0 (white).
    fn luma(&self, x: u32, y: u32) -> f64 {
        let i = ((y * self.width + x) * 4) as usize;
        let [r, g, b] = [
            self.pixels[i] as f64,
            self.pixels[i + 1] as f64,
            self.pixels[i + 2] as f64,
        ];
        (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255.0
    }

    /// Darkest luminance inside a logical-unit rect.
    fn darkest_in(&self, x0: f64, y0: f64, x1: f64, y1: f64) -> f64 {
        let s = self.frame.scale;
        let to_px = |v: f64, max: u32| (v * s).round().clamp(0.0, f64::from(max - 1)) as u32;
        let (px0, py0) = (to_px(x0, self.width), to_px(y0, self.height));
        let (px1, py1) = (to_px(x1, self.width), to_px(y1, self.height));
        let mut darkest = 1.0f64;
        for y in py0..=py1 {
            for x in px0..=px1 {
                darkest = darkest.min(self.luma(x, y));
            }
        }
        darkest
    }
}

fn nodes() -> Vec<(&'static str, Model)> {
    states::names()
        .into_iter()
        .map(|name| (name, states::by_name(name).expect("listed node")))
        .collect()
}

#[test]
#[ignore = "requires a GPU"]
fn nothing_draws_in_the_gap_above_the_composer() {
    for (name, model) in nodes() {
        let Some(r) = Rendered::new(&model) else {
            eprintln!("skipping {name}: no GPU");
            return;
        };
        let f = r.frame;
        // The band between the transcript and the well must stay paper:
        // this is the overlap bug that made long replies collide.
        let darkest = r.darkest_in(f.left, f.body_bottom + 2.0, f.right, f.composer_top - 2.0);
        assert!(
            darkest > 0.9,
            "{name}: ink ({darkest:.3} luma) in the composer gap"
        );
    }
}

#[test]
#[ignore = "requires a GPU"]
fn masthead_rule_is_clear_of_text() {
    for (name, model) in nodes() {
        let Some(r) = Rendered::new(&model) else {
            return;
        };
        let f = r.frame;
        // Just below the rule must be paper: status text that wraps past
        // its own rule was the second bug.
        let darkest = r.darkest_in(f.left, f.masthead_rule + 3.0, f.right, f.body_top - 3.0);
        assert!(darkest > 0.9, "{name}: text crossed the masthead rule");
    }
}

#[test]
#[ignore = "requires a GPU"]
fn body_text_has_readable_contrast() {
    for (name, model) in nodes() {
        let Some(r) = Rendered::new(&model) else {
            return;
        };
        let f = r.frame;
        // Some real ink must exist in the transcript band, dark enough to
        // read. Catches invisible text and silent layout collapse.
        let darkest = r.darkest_in(f.left, f.body_top, f.right, f.body_bottom);
        assert!(
            darkest < 0.65,
            "{name}: transcript is too faint to read (darkest {darkest:.3})"
        );
    }
}

/// The founding bug: layout in physical pixels with text laid out at 1x
/// made everything render tiny and blurry on a 1.75x display. Physical
/// text height must scale with the scale factor.
#[test]
#[ignore = "requires a GPU"]
fn text_is_rasterized_at_physical_size() {
    let model = states::by_name("turn_done").expect("node");
    const W: u32 = 1100;
    const H: u32 = 720;
    let Some(one) = Rendered::at(&model, W, H, 1.0) else {
        return;
    };
    let Some(two) = Rendered::at(&model, W * 2, H * 2, 2.0) else {
        return;
    };
    let f = one.frame;
    let base = one.ink_rows(f.left, f.body_top, f.right, f.body_bottom);
    let scaled = two.ink_rows(f.left, f.body_top, f.right, f.body_bottom);
    assert!(base > 0 && scaled > 0, "no text was drawn");
    let ratio = f64::from(scaled) / f64::from(base);
    assert!(
        (1.7..=2.3).contains(&ratio),
        "text did not scale with DPI: {base} rows at 1x vs {scaled} at 2x (ratio {ratio:.2})"
    );
}

/// A selection must be visible as a band, and the selected glyphs must
/// still be readable on top of it.
#[test]
#[ignore = "requires a GPU"]
fn a_selection_is_visible_and_text_on_it_stays_readable() {
    let model = states::by_name("selection").expect("node");
    let (start, end) = model.editor.selection().expect("node has a selection");
    assert!(start < end);
    let Some(r) = Rendered::new(&model) else {
        return;
    };
    let f = r.frame;
    let band_y = f.composer_top + crate::layout::COMPOSER_TEXT_OFFSET + 6.0;
    // Somewhere in the selection there must be a mid-tone band pixel that
    // is neither paper nor ink.
    let s = f.scale;
    let y = (band_y * s) as u32;
    let mut band_pixels = 0;
    let mut ink_pixels = 0;
    for x in ((f.left * s) as u32)..((f.right * s) as u32) {
        let luma = r.luma(x, y);
        if (0.55..0.95).contains(&luma) {
            band_pixels += 1;
        }
        if luma < 0.4 {
            ink_pixels += 1;
        }
    }
    assert!(band_pixels > 4, "no selection band was drawn");
    assert!(
        ink_pixels > 0,
        "selected text was hidden by the band instead of drawn on top"
    );
}

/// No selection means no band: otherwise the composer would always look
/// highlighted.
#[test]
#[ignore = "requires a GPU"]
fn no_band_is_drawn_without_a_selection() {
    let model = states::by_name("mid_input").expect("node");
    assert!(model.editor.selection().is_none());
    let Some(r) = Rendered::new(&model) else {
        return;
    };
    let f = r.frame;
    let s = f.scale;
    // Sample a row above the glyph bodies where a band would still paint.
    let y = ((f.composer_top + crate::layout::COMPOSER_TEXT_OFFSET + 1.0) * s) as u32;
    let band_pixels = (((f.left + 2.0) * s) as u32..((f.right - 2.0) * s) as u32)
        .filter(|&x| (0.55..0.95).contains(&r.luma(x, y)))
        .count();
    assert!(
        band_pixels < 10,
        "a selection band appeared without a selection ({band_pixels} px)"
    );
}

/// A multi-line message must actually render on multiple rows, with the
/// caret on the last line rather than the first.
#[test]
#[ignore = "requires a GPU"]
fn a_multiline_message_renders_on_multiple_rows() {
    let model = states::by_name("multiline").expect("node");
    assert!(model.editor.line_count() > 1);
    let Some(r) = Rendered::new(&model) else {
        return;
    };
    let f = r.frame;
    assert!(
        f.composer_lines() >= model.editor.line_count(),
        "the composer did not grow to fit the input"
    );
    // Each line's row must contain ink.
    let s = f.scale;
    for line in 0..model.editor.line_count() {
        let y = f.composer_top
            + crate::layout::COMPOSER_TEXT_OFFSET
            + line as f64 * crate::layout::COMPOSER_LINE_HEIGHT
            + 6.0;
        let row = (y * s) as u32;
        let inked = ((f.left * s) as u32..(f.right * s) as u32)
            .filter(|&x| r.luma(x, row) < 0.5)
            .count();
        assert!(inked > 0, "composer line {line} rendered nothing");
    }
}

/// A selection spanning a line break must highlight both lines.
#[test]
#[ignore = "requires a GPU"]
fn a_selection_across_lines_highlights_every_line() {
    let model = states::by_name("multiline_selection").expect("node");
    let Some(r) = Rendered::new(&model) else {
        return;
    };
    let f = r.frame;
    let s = f.scale;
    for line in 0..2 {
        let y = f.composer_top
            + crate::layout::COMPOSER_TEXT_OFFSET
            + line as f64 * crate::layout::COMPOSER_LINE_HEIGHT
            + 2.0;
        let row = (y * s) as u32;
        let band = ((f.left * s) as u32..(f.right * s) as u32)
            .filter(|&x| (0.55..0.95).contains(&r.luma(x, row)))
            .count();
        assert!(
            band > 4,
            "line {line} of a multi-line selection was not highlighted"
        );
    }
}

/// The founding bug for wrapping: a long line rendered past the right edge of
/// the well. Nothing may be drawn outside the composer.
#[test]
#[ignore = "requires a GPU"]
fn a_long_line_wraps_inside_the_composer_well() {
    let model = states::by_name("wrapped_long_line").expect("node");
    assert_eq!(
        model.editor.line_count(),
        1,
        "node should be one logical line"
    );
    let Some(r) = Rendered::new(&model) else {
        return;
    };
    let f = r.frame;
    assert!(
        f.composer_lines() > 1,
        "the well did not grow to fit the wrapped text"
    );
    // No ink right of the column, and none between the well and the footnote.
    let right = r.darkest_in(
        f.right + 1.0,
        f.composer_top,
        f.width - 1.0,
        f.composer_bottom,
    );
    assert!(
        right > 0.9,
        "wrapped text ran past the right edge ({right:.3})"
    );
    let below = r.darkest_in(
        f.left,
        f.composer_bottom + 1.0,
        f.right,
        f.footnote_top - 1.0,
    );
    assert!(
        below > 0.9,
        "wrapped text spilled below the well ({below:.3})"
    );
    // Every visible row must actually carry text.
    let s = f.scale;
    for row in 0..f.composer_lines().min(3) {
        let y = f.composer_top
            + crate::layout::COMPOSER_TEXT_OFFSET
            + row as f64 * crate::layout::COMPOSER_LINE_HEIGHT
            + 6.0;
        let inked = ((f.left * s) as u32..(f.right * s) as u32)
            .filter(|&x| r.luma(x, (y * s) as u32) < 0.5)
            .count();
        assert!(inked > 0, "wrapped row {row} rendered nothing");
    }
}

/// The caret must sit on the row that owns the cursor. Drawing it on the first
/// row would look plausible on short input and be wrong on every wrapped line.
#[test]
#[ignore = "requires a GPU"]
fn the_caret_sits_on_the_cursor_row_when_wrapped() {
    let mut model = states::by_name("wrapped_long_line").expect("node");
    model.caret = crate::caret::Caret::pinned(true);
    // Cursor at the end: the caret belongs on the last visible row.
    model.editor.move_end();
    let Some(r) = Rendered::new(&model) else {
        return;
    };
    let f = r.frame;
    let rows = f.composer_lines();
    assert!(rows > 1, "node did not wrap");

    // A caret is a full-height bar, so its row has ink spanning the sampled
    // band. Find which row carries a bar past the end of that row's text.
    let s = f.scale;
    let row_band = |row: usize| {
        let top = f.composer_top
            + crate::layout::COMPOSER_TEXT_OFFSET
            + row as f64 * crate::layout::COMPOSER_LINE_HEIGHT;
        let y0 = ((top + 2.0) * s) as u32;
        let y1 = ((top + 12.0) * s) as u32;
        (y0, y1)
    };
    let bar_columns = |row: usize| {
        let (y0, y1) = row_band(row);
        (((f.left + crate::layout::COMPOSER_PAD_X) * s) as u32..((f.right - 2.0) * s) as u32)
            .filter(|&x| (y0..=y1).all(|y| r.luma(x, y) < 0.5))
            .count()
    };
    let last = rows - 1;
    assert!(
        bar_columns(last) > 0,
        "no caret bar on the last row, where the cursor is"
    );

    // Now put the cursor on the first row and confirm the bar moves there.
    let mut first_row_model = states::by_name("wrapped_long_line").expect("node");
    first_row_model.caret = crate::caret::Caret::pinned(true);
    first_row_model.editor.place_cursor(3);
    let Some(r2) = Rendered::new(&first_row_model) else {
        return;
    };
    let caret_y_of = |rendered: &Rendered| {
        let f = rendered.frame;
        let s = f.scale;
        // The topmost inked row inside the well that has a full-height bar.
        (0..f.composer_lines()).find(|&row| {
            let top = f.composer_top
                + crate::layout::COMPOSER_TEXT_OFFSET
                + row as f64 * crate::layout::COMPOSER_LINE_HEIGHT;
            let y0 = ((top + 2.0) * s) as u32;
            let y1 = ((top + 12.0) * s) as u32;
            (((f.left + crate::layout::COMPOSER_PAD_X) * s) as u32..((f.right - 2.0) * s) as u32)
                .any(|x| (y0..=y1).all(|y| rendered.luma(x, y) < 0.5))
        })
    };
    let first_caret_row = caret_y_of(&r2);
    assert_eq!(
        first_caret_row,
        Some(0),
        "a cursor on the first row did not draw its caret there"
    );
}

/// A node must render identically no matter when it is rendered, or every
/// pixel test becomes timing-dependent and flaky.
#[test]
#[ignore = "requires a GPU"]
fn state_nodes_render_deterministically() {
    for (name, model) in nodes() {
        let Some(first) = Rendered::new(&model) else {
            return;
        };
        std::thread::sleep(std::time::Duration::from_millis(700));
        let Some(second) = Rendered::new(&model) else {
            return;
        };
        assert!(
            first.pixels == second.pixels,
            "{name} rendered differently 700ms later (time-dependent frame)"
        );
    }
}

/// Columns of ink inside the composer well, as physical x positions.
/// Used to find the caret without knowing font metrics.
fn caret_columns(r: &Rendered) -> Vec<u32> {
    let f = r.frame;
    let s = f.scale;
    let y0 = ((f.composer_top + crate::layout::COMPOSER_TEXT_OFFSET + 2.0) * s) as u32;
    let y1 = ((f.composer_top + crate::layout::COMPOSER_TEXT_OFFSET + 12.0) * s) as u32;
    let x0 = (f.left * s) as u32;
    let x1 = (f.right * s) as u32;
    (x0..x1)
        .filter(|&x| (y0..=y1).all(|y| r.luma(x, y) < 0.5))
        .collect()
}

/// A caret is a full-height vertical bar, so it inks every sampled row in
/// its column. Empty input has no glyphs, so any such column is the caret.
#[test]
#[ignore = "requires a GPU"]
fn an_insert_caret_is_drawn_in_the_empty_composer() {
    let model = states::by_name("attached_empty").expect("node");
    let Some(r) = Rendered::new(&model) else {
        return;
    };
    let columns = caret_columns(&r);
    assert!(
        !columns.is_empty(),
        "no insert caret was drawn in the empty composer"
    );
    let f = r.frame;
    let expected = ((f.left + crate::layout::COMPOSER_PAD_X) * f.scale) as u32;
    assert!(
        columns.iter().any(|&x| x.abs_diff(expected) <= 4),
        "caret was not at the start of the empty input (columns {:?}, expected ~{expected})",
        &columns[..columns.len().min(8)]
    );
}

/// The caret must track the cursor index, which is what makes this a real
/// input box rather than a trailing underscore. Compared against a caret
/// rendered on the *same* text with the cursor at the end, so the only
/// difference is the cursor position.
#[test]
#[ignore = "requires a GPU"]
fn the_caret_moves_with_the_cursor() {
    let mut inside = states::by_name("mid_input_caret_inside").expect("node");
    let mut at_end = states::by_name("mid_input_caret_inside").expect("node");
    at_end.editor.set_cursor_public(at_end.editor.text().len());
    // Same text, same node, different cursor.
    assert_eq!(inside.editor.text(), at_end.editor.text());
    assert!(inside.editor.cursor() < at_end.editor.cursor());
    inside.caret = crate::caret::Caret::pinned(true);
    at_end.caret = crate::caret::Caret::pinned(true);

    let Some(a) = Rendered::new(&inside) else {
        return;
    };
    let Some(b) = Rendered::new(&at_end) else {
        return;
    };
    let mid = caret_columns(&a);
    let tail = caret_columns(&b);
    assert!(!mid.is_empty(), "no caret drawn with the cursor mid-text");
    assert!(
        !tail.is_empty(),
        "no caret drawn with the cursor at the end"
    );
    let mid_x = *mid.iter().max().expect("columns");
    let tail_x = *tail.iter().max().expect("columns");
    assert!(
        tail_x > mid_x + 20,
        "caret did not follow the cursor: mid-text at {mid_x}, at end {tail_x}"
    );
}

/// The blink must actually blink: the off phase draws no caret.
#[test]
#[ignore = "requires a GPU"]
fn the_caret_disappears_on_the_blink_off_phase() {
    let hidden = states::by_name("caret_hidden").expect("node");
    assert!(
        !hidden.caret.visible(),
        "the caret_hidden node is not actually in an off phase"
    );
    let Some(r) = Rendered::new(&hidden) else {
        return;
    };
    // Sample past the end of the text, where only a caret could ink.
    let f = r.frame;
    let text_end = f.left + crate::layout::COMPOSER_PAD_X + 200.0;
    let darkest = r.darkest_in(
        text_end,
        f.composer_top + 4.0,
        f.right - 2.0,
        f.composer_bottom - 4.0,
    );
    assert!(
        darkest > 0.85,
        "something was drawn past the text on the blink off phase ({darkest:.3})"
    );
}

/// The caret must never escape its well, at any window size.
#[test]
#[ignore = "requires a GPU"]
fn the_caret_stays_inside_the_composer_well() {
    for (name, model) in nodes() {
        let Some(r) = Rendered::new(&model) else {
            return;
        };
        let f = r.frame;
        // Bands immediately above and below the well must stay paper.
        let above = r.darkest_in(f.left, f.composer_top - 6.0, f.right, f.composer_top - 2.0);
        assert!(above > 0.9, "{name}: ink just above the composer well");
        let below = r.darkest_in(
            f.left,
            f.composer_bottom + 1.0,
            f.right,
            f.footnote_top - 1.0,
        );
        assert!(below > 0.9, "{name}: ink between the well and the footnote");
    }
}

#[test]
#[ignore = "requires a GPU"]
fn margins_stay_empty() {
    for (name, model) in nodes() {
        let Some(r) = Rendered::new(&model) else {
            return;
        };
        let f = r.frame;
        // Nothing may be drawn outside the measure column: proves text is
        // wrapped to the column and not clipped by the window edge.
        let left_margin = r.darkest_in(0.0, 0.0, f.left - 3.0, f.height - 1.0);
        assert!(left_margin > 0.9, "{name}: ink in the left margin");
        let bottom = r.darkest_in(0.0, f.footnote_bottom + 2.0, f.width - 1.0, f.height - 1.0);
        assert!(bottom > 0.9, "{name}: ink below the footnote row");
    }
}
