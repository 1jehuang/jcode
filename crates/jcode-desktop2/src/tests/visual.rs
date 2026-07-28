//! Pixel-level visual invariants: render every state-space node offscreen and
//! assert what only real output can prove (regions stay clear, text is legible,
//! the caret and selection land where they should). These need a GPU, so they
//! are `#[ignore]`d; run with `cargo test -p jcode-desktop2 -- --ignored`.

use crate::{Model, build_scene, layout::Frame, states, text::TextSystem};
use vello::Scene;

const WIDTH: u32 = 1400;
const HEIGHT: u32 = 900;
const SCALE: f64 = 1.75;

pub(super) struct Rendered {
    pixels: Vec<u8>,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(crate) frame: Frame,
}

impl Rendered {
    pub(super) fn new(model: &Model) -> Option<Self> {
        Self::at(model, WIDTH, HEIGHT, SCALE)
    }

    /// Render one model at an explicit surface size and scale factor.
    pub(super) fn at(model: &Model, width: u32, height: u32, scale: f64) -> Option<Self> {
        let mut painter = crate::paint::Painter::default();
        let mut scene = Scene::new();
        build_scene(&mut scene, &mut painter, model, (width, height), scale);
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
    pub(super) fn ink_rows(&self, x0: f64, y0: f64, x1: f64, y1: f64) -> u32 {
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
    pub(super) fn luma(&self, x: u32, y: u32) -> f64 {
        let i = ((y * self.width + x) * 4) as usize;
        let [r, g, b] = [
            self.pixels[i] as f64,
            self.pixels[i + 1] as f64,
            self.pixels[i + 2] as f64,
        ];
        (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255.0
    }

    /// Darkest luminance inside a logical-unit rect.
    pub(super) fn darkest_in(&self, x0: f64, y0: f64, x1: f64, y1: f64) -> f64 {
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

    /// Mean luminance inside a logical-unit rect. Sensitive to ink spread
    /// across a whole region, unlike `darkest_in`, which reports only the
    /// single most opaque pixel and so cannot tell bold from plain.
    fn mean_in(&self, x0: f64, y0: f64, x1: f64, y1: f64) -> f64 {
        let s = self.frame.scale;
        let to_px = |v: f64, max: u32| (v * s).round().clamp(0.0, f64::from(max - 1)) as u32;
        let (px0, py0) = (to_px(x0, self.width), to_px(y0, self.height));
        let (px1, py1) = (to_px(x1, self.width), to_px(y1, self.height));
        let mut total = 0.0;
        let mut count = 0u32;
        for y in py0..=py1 {
            for x in px0..=px1 {
                total += self.luma(x, y);
                count += 1;
            }
        }
        if count == 0 {
            1.0
        } else {
            total / f64::from(count)
        }
    }

    /// Vertical extent, in logical units, of the composer wash as actually
    /// drawn. Sampled on a column just inside the right edge of the measure
    /// column, where only the well can ink, so prompt glyphs cannot be
    /// mistaken for the well itself.
    pub(super) fn wash_band(&self) -> Option<(f64, f64)> {
        // The composer is an outlined field, so it is found by its horizontal
        // border rules rather than by a fill: scan a column just inside the
        // right edge (where only the field's own borders can ink) and take the
        // first and last inked rows below the transcript.
        let s = self.frame.scale;
        let x = ((self.frame.right - 8.0) * s).round() as u32;
        let start = ((self.frame.body_top + 6.0) * s).round() as u32;
        let mut first = None;
        let mut last = None;
        for y in start..self.height {
            if self.luma(x, y) < 0.95 {
                first = first.or(Some(y));
                last = Some(y);
            }
        }
        Some((f64::from(first?) / s, f64::from(last?) / s))
    }
}

pub(super) fn nodes() -> Vec<(&'static str, Model)> {
    states::names()
        .into_iter()
        .map(|name| (name, states::by_name(name).expect("listed node")))
        .collect()
}

/// The highlight band must line up with the glyphs it highlights. This is the
/// bug the Parley geometry fixed: the band came from a separately measured
/// string prefix, so it sat a few pixels off the selected text. Here the band's
/// horizontal extent is compared against the ink it is supposed to cover.
#[test]
#[ignore = "requires a GPU"]
fn the_selection_band_lines_up_with_the_selected_glyphs() {
    for name in ["selection", "selection_all", "multiline_selection"] {
        let model = states::by_name(name).expect("node");
        let (start, end) = model.editor.selection().expect("node has a selection");
        let Some(r) = Rendered::new(&model) else {
            return;
        };
        let f = r.frame;
        let s = f.scale;
        let band = model.theme.selection;
        let band_luma = (0.2126 * f64::from(band.components[0])
            + 0.7152 * f64::from(band.components[1]))
            + 0.0722 * f64::from(band.components[2]);
        assert!(band_luma > 0.0, "{name}: selection colour is not set");

        // Columns inside the well that are neither paper nor pure glyph ink:
        // the band tints them. Sample a row clear of glyph ascenders.
        let y = ((f.composer_top + crate::layout::COMPOSER_TEXT_OFFSET + 3.0) * s).round() as u32;
        let x0 = (f.composer_text_left() * s) as u32;
        let x1 = ((f.right - 6.0) * s) as u32;
        let paper = r.luma(x1 - 2, y);
        let tinted: Vec<u32> = (x0..x1).filter(|&x| paper - r.luma(x, y) > 0.01).collect();
        assert!(
            !tinted.is_empty(),
            "{name}: no selection band was drawn at all"
        );

        // The band must start at the caret for the selection start and end at
        // the caret for its end: same geometry the renderer used, re-derived.
        let mut ts = TextSystem::default();
        let input = crate::input::InputLayout::new(
            &mut ts,
            model.editor.text(),
            f.composer_text_width(),
            crate::scene::composer_text_style(&model),
            f.scale,
        );
        let text_x = f.composer_text_left();
        let expected_start = ((text_x + input.caret_rect(start, 1.0).x0) * s).round() as u32;
        let expected_end = ((text_x + input.caret_rect(end, 1.0).x0) * s).round() as u32;
        let drawn_start = *tinted.first().expect("checked non-empty");
        // Only antialiasing slack: a band that is even a logical pixel off the
        // caret is the misalignment this test exists to catch.
        let slack = s.ceil() as u32 + 1;
        assert!(
            drawn_start.abs_diff(expected_start) <= slack,
            "{name}: band started at {drawn_start}px, expected {expected_start}px"
        );
        // On the first row of a multi-line selection the band runs to the row
        // end, so only check the end when the selection stays on one row.
        if input.lines().len() == 1 {
            let drawn_end = *tinted.last().expect("checked non-empty");
            assert!(
                drawn_end.abs_diff(expected_end) <= slack,
                "{name}: band ended at {drawn_end}px, expected {expected_end}px"
            );
        }
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
        // An empty session draws no transcript at all (the hero, or nothing, is
        // there instead), so there is no ink to hold to a contrast floor.
        if model.transcript.is_empty() {
            continue;
        }
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
    for x in ((f.composer_text_left() * s) as u32)..(((f.right - 4.0) * s) as u32) {
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
    let band_pixels = ((f.composer_text_left() * s) as u32..((f.right - 6.0) * s) as u32)
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
        let inked = (((f.composer_text_left()) * s) as u32..((f.right - 4.0) * s) as u32)
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
        let band = (((f.composer_text_left()) * s) as u32..((f.right - 4.0) * s) as u32)
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
        let inked = (((f.composer_text_left()) * s) as u32..((f.right - 4.0) * s) as u32)
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
        (((f.composer_text_left()) * s) as u32..((f.right - 2.0) * s) as u32)
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
            (((f.composer_text_left()) * s) as u32..((f.right - 2.0) * s) as u32)
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
///
/// "Identically" allows single least-significant-bit wobble on a handful of
/// pixels: Vello rasterizes with GPU atomics, whose accumulation order is not
/// deterministic, and on this class of hardware two renders of the same scene
/// occasionally disagree by one 8-bit step on one antialiased edge pixel.
/// That is GPU noise, not a time-dependent frame; a real clock leak (a
/// spinner frame, a blink phase, a breath) moves whole glyphs and hundreds of
/// pixels by far more than one step.
#[test]
#[ignore = "requires a GPU"]
fn state_nodes_render_deterministically() {
    const MAX_WOBBLE_PIXELS: usize = 8;
    for (name, model) in nodes() {
        let Some(first) = Rendered::new(&model) else {
            return;
        };
        std::thread::sleep(std::time::Duration::from_millis(700));
        let Some(second) = Rendered::new(&model) else {
            return;
        };
        let pairs = first.pixels.iter().zip(&second.pixels);
        let wobble = pairs.clone().filter(|(a, b)| a != b).count();
        let worst = pairs.map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0);
        assert!(
            worst <= 1 && wobble <= MAX_WOBBLE_PIXELS,
            "{name} rendered differently 700ms later (time-dependent frame: \
             {wobble} bytes changed, worst step {worst})"
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
    // Inside the field: its own left/right borders are full-height inked
    // columns and would otherwise be mistaken for the caret.
    let x0 = (f.composer_text_left() * s) as u32;
    let x1 = ((f.right - 4.0) * s) as u32;
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
    let expected = ((f.composer_text_left()) * f.scale) as u32;
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
    let text_end = f.composer_text_left() + 200.0;
    let darkest = r.darkest_in(
        text_end,
        f.composer_top + 4.0,
        f.right - 6.0,
        f.composer_bottom - 4.0,
    );
    assert!(
        darkest > 0.85,
        "something was drawn past the text on the blink off phase ({darkest:.3})"
    );
}

/// A model that is not attached must still say so somewhere, or a dead
/// runtime is indistinguishable from an app that ignores input. With the
/// masthead gone, that somewhere is the footnote row.
#[test]
#[ignore = "requires a GPU"]
fn an_unattached_state_still_reports_its_status() {
    let model = crate::states::by_name("connecting").expect("connecting node");
    assert!(
        model.footnote().is_some(),
        "an unattached model reported no status at all"
    );
    let Some(r) = Rendered::new(&model) else {
        eprintln!("skipping: no GPU");
        return;
    };
    let f = r.frame;
    let darkest = r.darkest_in(f.left, f.footnote_top, f.right, f.footnote_bottom);
    assert!(
        darkest < 0.9,
        "the connection status was not drawn in the footnote row ({darkest:.3})"
    );
}

/// Not an assertion: writes frames to `JCODE_DESKTOP2_DUMP` for eyeballing.
/// Ignored, so it only runs when a human asks for pictures.
#[test]
#[ignore = "writes files for review"]
fn dump_frames_for_review() {
    let Some(dir) = std::env::var_os("JCODE_DESKTOP2_DUMP") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir).expect("create dump dir");
    let mut painter = crate::paint::Painter::default();
    for (name, model) in nodes() {
        let mut scene = Scene::new();
        build_scene(&mut scene, &mut painter, &model, (WIDTH, HEIGHT), SCALE);
        let path = dir.join(format!("{name}.png"));
        crate::capture::capture_scene_to_png(&scene, WIDTH, HEIGHT, &path).expect("write png");
        eprintln!("wrote {}", path.display());
    }
}

/// Composer text must sit vertically centred in its well: equal paper above
/// and below the ink. A hardcoded text offset left it a pixel high, which is
/// exactly the kind of drift that makes an input box look wrong.
#[test]
#[ignore = "requires a GPU"]
fn composer_text_is_vertically_centred_in_the_well() {
    for name in ["mid_input", "attached_empty", "selection", "multiline"] {
        let model = states::by_name(name).expect("node");
        let Some(r) = Rendered::new(&model) else {
            return;
        };
        let f = r.frame;
        let s = f.scale;
        let x0 = ((f.composer_text_left()) * s) as u32;
        let x1 = (f.right * s) as u32 - 2;
        let mut first = None;
        let mut last = None;
        for y in ((f.composer_top * s) as u32)..((f.composer_bottom * s) as u32) {
            if (x0..x1).any(|x| r.luma(x, y) < 0.5) {
                first = first.or(Some(y));
                last = Some(y);
            }
        }
        let a = first.expect("no composer ink was drawn");
        let b = last.expect("no composer ink was drawn");
        let lines = model.editor.line_count();
        let above = f64::from(a) / s - f.composer_top;
        let below = f.composer_bottom - f64::from(b) / s;
        // Glyph ink is not symmetric about its line box: the cap height leaves
        // more room above than the descender leaves below, and that gap grows
        // with the number of lines because only the outer lines are measured.
        // Allow for that, but not for the padding itself being wrong, which is
        // what a hardcoded text offset got wrong by a whole line-box pixel.
        let budget = if lines > 1 { 4.0 } else { 1.5 };
        assert!(
            (above - below).abs() <= budget,
            "{name}: composer text is off-centre: {above:.1}px above, {below:.1}px below"
        );
    }
}

/// The composer must read as an input field: a bordered box, not a shaded
/// slab. So the field interior must be the same paper as the page, and the
/// border must be a thin inked outline on all four edges. A filled well
/// passes neither check.
#[test]
#[ignore = "requires a GPU"]
fn the_composer_is_an_outlined_field_not_a_filled_slab() {
    for name in ["mid_input", "attached_empty", "multiline"] {
        let model = states::by_name(name).expect("node");
        let Some(r) = Rendered::new(&model) else {
            return;
        };
        let f = r.frame;
        let s = f.scale;
        // Interior, clear of the marker/text column and of the border.
        let interior = r.darkest_in(
            f.right - 40.0,
            f.composer_top + 6.0,
            f.right - 6.0,
            f.composer_bottom - 6.0,
        );
        let page = r.darkest_in(
            f.right - 40.0,
            f.body_top + 4.0,
            f.right - 6.0,
            f.body_top + 10.0,
        );
        assert!(
            interior > 0.97,
            "{name}: the field interior is tinted ({interior:.3}); it should be paper"
        );
        assert!(page > 0.9, "unexpected ink in the sampled page band");

        // Border: each edge must ink, midway along that edge.
        let mid_x = ((f.left + f.right) / 2.0 * s) as u32;
        let mid_y = ((f.composer_top + f.composer_bottom) / 2.0 * s) as u32;
        let edges = [
            ("top", mid_x, (f.composer_top * s).round() as u32),
            ("bottom", mid_x, (f.composer_bottom * s).round() as u32),
            ("left", (f.left * s).round() as u32, mid_y),
            ("right", (f.right * s).round() as u32, mid_y),
        ];
        for (edge, x, y) in edges {
            // Allow a pixel of slack for stroke centring and antialiasing.
            let inked = (-2i64..=2).any(|d| {
                let (px, py) = if edge == "top" || edge == "bottom" {
                    (x, (y as i64 + d).max(0) as u32)
                } else {
                    ((x as i64 + d).max(0) as u32, y)
                };
                px < r.width && py < r.height && r.luma(px, py) < 0.95
            });
            assert!(inked, "{name}: no {edge} border was drawn on the field");
        }
    }
}

/// Focus must be visible: the focused border is stronger than the unfocused
/// one. Without this the field looks identical whether or not keystrokes will
/// land in it.
#[test]
#[ignore = "requires a GPU"]
fn the_field_border_shows_focus() {
    let mut focused = states::by_name("mid_input").expect("node");
    focused.focused = true;
    let mut blurred = states::by_name("mid_input").expect("node");
    blurred.focused = false;
    let Some(a) = Rendered::new(&focused) else {
        return;
    };
    let Some(b) = Rendered::new(&blurred) else {
        return;
    };
    let f = a.frame;
    // Sample the top border band on both, away from the text.
    let band = |r: &Rendered| {
        r.darkest_in(
            f.right - 60.0,
            f.composer_top - 1.0,
            f.right - 10.0,
            f.composer_top + 1.0,
        )
    };
    let focused_ink = band(&a);
    let blurred_ink = band(&b);
    assert!(
        focused_ink < blurred_ink - 0.05,
        "focus is invisible: focused border {focused_ink:.3} vs unfocused {blurred_ink:.3}"
    );
}

/// An unfocused window must not blink a caret, or it claims keystrokes it will
/// not receive.
#[test]
#[ignore = "requires a GPU"]
fn no_caret_is_drawn_while_unfocused() {
    let model = states::by_name("unfocused").expect("node");
    assert!(!model.focused, "the unfocused node is focused");
    assert!(
        model.caret.visible(),
        "node pins the caret on, to prove focus gates it"
    );
    let Some(r) = Rendered::new(&model) else {
        return;
    };
    let f = r.frame;
    // Past the end of the text, inside the field: only a caret could ink here.
    let darkest = r.darkest_in(
        f.composer_text_left() + 220.0,
        f.composer_top + 4.0,
        f.right - 4.0,
        f.composer_bottom - 4.0,
    );
    assert!(
        darkest > 0.9,
        "a caret was drawn in an unfocused field ({darkest:.3})"
    );
}

/// While busy, anything already typed for the next turn must stay visible.
/// The old design replaced the whole field with a "working..." label, silently
/// hiding queued input.
#[test]
#[ignore = "requires a GPU"]
fn typed_text_survives_the_busy_state() {
    let mut model = states::by_name("mid_input").expect("node");
    model.busy = true;
    let Some(r) = Rendered::new(&model) else {
        return;
    };
    let f = r.frame;
    let darkest = r.darkest_in(
        f.composer_text_left(),
        f.composer_top + 3.0,
        f.right - 4.0,
        f.composer_bottom - 3.0,
    );
    assert!(
        darkest < 0.6,
        "the busy state hid text typed for the next turn ({darkest:.3})"
    );
}

/// The model caption must actually reach the pixels, on the trailing end of the
/// footnote row, and must not collide with a footnote sharing that row. A
/// unit-tested label that the renderer forgets to draw is the failure mode this
/// guards. Thresholds are luminance-based: the caption is deliberately faint,
/// so a strict ink test would report an absence that is really just low
/// contrast.
#[test]
#[ignore = "requires a GPU"]
fn the_model_caption_is_drawn_on_the_right_of_the_footnote_row() {
    let mut model = states::by_name("attached_empty").expect("node");
    model.notice = None;
    model.model = Some(crate::ModelId {
        provider: Some("anthropic".into()),
        model: Some("claude-sonnet-4-5".into()),
    });
    assert!(
        model.footnote().is_none(),
        "this case wants the caption alone on the row"
    );
    let Some(shot) = Rendered::new(&model) else {
        eprintln!("skipping: no GPU");
        return;
    };
    let f = shot.frame;
    let mid = (f.left + f.right) / 2.0;
    let top = f.footnote_top;
    let bottom = f.footnote_bottom;
    assert!(
        shot.darkest_in(mid, top, f.right, bottom) < 0.9,
        "no model caption on the right of the footnote row"
    );

    // With no footnote to share the row, the left half must stay clear, so the
    // caption reads as trailing metadata rather than drifting into the middle.
    assert!(
        shot.darkest_in(f.left, top, mid - 4.0, bottom) > 0.95,
        "the model caption is not right-aligned"
    );
}

/// A model caption and a footnote must coexist without overlapping: both are
/// elided to fit their own half of the row.
#[test]
#[ignore = "requires a GPU"]
fn a_footnote_and_the_model_caption_do_not_collide() {
    let mut model = states::by_name("attached_empty").expect("node");
    model.notice = Some("nothing to undo".into());
    model.model = Some(crate::ModelId {
        provider: Some("anthropic".into()),
        model: Some("claude-sonnet-4-5".into()),
    });
    assert!(
        model.footnote().is_some(),
        "this case wants both captions on the row"
    );
    let Some(shot) = Rendered::new(&model) else {
        eprintln!("skipping: no GPU");
        return;
    };
    let f = shot.frame;
    let top = f.footnote_top;
    let bottom = f.footnote_bottom;
    let mid = (f.left + f.right) / 2.0;
    assert!(
        shot.darkest_in(f.left, top, mid - 8.0, bottom) < 0.9,
        "the footnote vanished when a model caption shared the row"
    );
    assert!(
        shot.darkest_in(mid + 8.0, top, f.right, bottom) < 0.9,
        "the model caption vanished when a footnote shared the row"
    );
    // A gutter in the middle proves neither ran into the other.
    assert!(
        shot.darkest_in(mid - 6.0, top, mid + 6.0, bottom) > 0.95,
        "the footnote and the model caption ran together"
    );
}

/// A single reply too tall for the transcript region must be clipped to it,
/// not painted straight down over the composer. Before the transcript was
/// clipped, the fit loop could only drop whole logical lines, so one long
/// streamed paragraph (exactly what a real answer looks like) wrapped past
/// `body_bottom` and struck through the input field.
#[test]
#[ignore = "requires a GPU"]
fn an_overflowing_reply_stays_out_of_the_composer() {
    let model = states::by_name("long_paragraph").expect("the overflow node");
    let Some(r) = Rendered::new(&model) else {
        eprintln!("skipping: no GPU");
        return;
    };
    let f = r.frame;
    // Inside the well, past where the placeholder hint and the caret can reach:
    // the overflowing paragraph fills the whole measure column, so if it leaked
    // into the field it inks here. Sampled between the horizontal borders so
    // the field's own outline is not mistaken for transcript ink.
    let darkest = r.darkest_in(
        f.left + f.column() * 0.75,
        f.composer_top + 3.0,
        f.right - 4.0,
        f.composer_bottom - 3.0,
    );
    assert!(
        darkest > 0.55,
        "transcript ink ({darkest:.3} luma) landed inside the composer well"
    );
    // And nothing below the well either.
    let below = r.darkest_in(f.left, f.footnote_bottom + 4.0, f.right, f.height - 2.0);
    assert!(
        below > 0.9,
        "transcript ink ({below:.3} luma) spilled below the footnote row"
    );
}

/// Markdown must arrive as *typography*, not as punctuation. The strongest
/// pixel-level evidence is that bold text inks more than the same text plain:
/// a renderer that dropped the ranged weight would pass every string-level
/// test while drawing a uniform grey paragraph.
#[test]
#[ignore = "requires a GPU"]
fn bold_markdown_inks_more_than_plain_text() {
    use crate::transcript::{Message, Transcript};

    let plain = Model {
        transcript: {
            let mut t = Transcript::default();
            t.push(Message::assistant("line-delimited JSON"));
            t
        },
        ..states::by_name("attached_empty").expect("base node")
    };
    let bold = Model {
        transcript: {
            let mut t = Transcript::default();
            t.push(Message::assistant("**line-delimited JSON**"));
            t
        },
        ..states::by_name("attached_empty").expect("base node")
    };
    let (Some(a), Some(b)) = (Rendered::new(&plain), Rendered::new(&bold)) else {
        eprintln!("skipping: no GPU");
        return;
    };
    let f = a.frame;
    let plain_ink = a.mean_in(f.left, f.body_top, f.right, f.body_bottom);
    let bold_ink = b.mean_in(f.left, f.body_top, f.right, f.body_bottom);
    assert!(
        bold_ink < plain_ink,
        "bold text did not ink more than plain ({bold_ink:.4} vs {plain_ink:.4}); \
         the ranged weight was probably dropped"
    );
}

/// A user message must be visually distinct from a reply *without* a marker
/// glyph. The card's tint is the distinction, so the user's band must be
/// measurably darker than paper while its text stays readable.
#[test]
#[ignore = "requires a GPU"]
fn a_user_message_reads_as_a_card_rather_than_a_marker() {
    let model = states::by_name("turn_done").expect("the turn_done node");
    let Some(r) = Rendered::new(&model) else {
        eprintln!("skipping: no GPU");
        return;
    };
    let f = r.frame;
    // The user's card is the topmost placed message, so sample the right-hand
    // end of the first inked band, past where its short text reaches: that is
    // card tint if it is anything.
    let mut card_row = None;
    for row in 0..((f.body_bottom - f.body_top) as u32) {
        let y = f.body_top + f64::from(row);
        if r.mean_in(f.right - 60.0, y, f.right - 10.0, y + 1.0) < 0.99 {
            card_row = Some(y);
            break;
        }
    }
    let y = card_row.expect("no tinted card band found; the user card is missing");
    let tint = r.mean_in(f.right - 60.0, y + 2.0, f.right - 10.0, y + 8.0);
    assert!(
        tint < 0.99,
        "the user message has no tint, so only a marker could distinguish it"
    );
    assert!(
        tint > 0.85,
        "the user card tint ({tint:.3}) is heavy enough to fight the text on it"
    );
}

/// Every rich node must keep its ink inside the transcript region. This is the
/// general form of the overflow test: markdown, math, and code all change the
/// measured height, so each is a fresh chance to overflow.
#[test]
#[ignore = "requires a GPU"]
fn rich_content_never_inks_the_composer() {
    for name in ["markdown", "latex", "code_block", "scrolled_back"] {
        let model = states::by_name(name).expect("node");
        let Some(r) = Rendered::new(&model) else {
            eprintln!("skipping: no GPU");
            return;
        };
        let f = r.frame;
        // Between the field's borders, right of where the hint reaches.
        let darkest = r.darkest_in(
            f.left + f.column() * 0.8,
            f.composer_top + 3.0,
            f.right - 4.0,
            f.composer_bottom - 3.0,
        );
        assert!(
            darkest > 0.55,
            "{name}: transcript ink ({darkest:.3} luma) landed inside the composer"
        );
    }
}

/// The busy line starts at the composer's text margin, with no spinner drawn
/// before it. The elapsed clock in the line itself is the liveness signal, so
/// the phase text must reach the pixels right at the margin: a working turn
/// that renders nothing there looks frozen.
#[test]
#[ignore = "requires a GPU"]
fn the_busy_line_starts_at_the_text_margin_with_no_spinner() {
    let model = states::by_name("working").expect("node");
    let Some(r) = Rendered::new(&model) else {
        return;
    };
    let f = r.frame;
    // The phase text starts right at the margin...
    let darkest_text = r.darkest_in(
        f.composer_text_left(),
        f.composer_top + 3.0,
        f.composer_text_left() + 80.0,
        f.composer_bottom - 3.0,
    );
    assert!(
        darkest_text < 0.92,
        "no busy-line ink at the text margin ({darkest_text:.3}), so a working turn looks frozen"
    );
    // ...and the band to its left stays clean, or the spinner is back.
    let darkest_left = r.darkest_in(
        f.left + 2.0,
        f.composer_top + 3.0,
        f.composer_text_left() - 1.0,
        f.composer_bottom - 3.0,
    );
    assert!(
        darkest_left > 0.9,
        "ink before the busy line ({darkest_left:.3}), so the spinner is back"
    );
}

/// The streaming reveal must actually hold text back, and must converge on the
/// same frame the non-streaming path draws.
///
/// This is the one thing unit tests over [`crate::stream`] cannot prove: the
/// arithmetic could be perfect while the renderer ignored it entirely, which is
/// exactly the bug an "animation" is most likely to ship with.
mod streaming {
    use super::*;

    fn model_with_reply(fraction: Option<f64>) -> Model {
        let mut model = states::by_name("attached_empty").expect("attached_empty node");
        model.transcript = crate::transcript::Transcript::default();
        model
            .transcript
            .push(crate::transcript::Message::user("hi"));
        model.transcript.append_assistant(
            "This is a streamed reply long enough to wrap across several \
             lines of the transcript column, so a partial reveal is visible \
             as missing text rather than as a single missing word.",
        );
        model.donut = None;
        model.stream = match fraction {
            Some(fraction) => crate::stream::Stream::pinned(fraction),
            None => crate::stream::Stream::default(),
        };
        model
    }

    fn body_ink(rendered: &Rendered) -> u32 {
        let frame = rendered.frame;
        rendered.ink_rows(frame.left, frame.body_top, frame.right, frame.body_bottom)
    }

    #[test]
    #[ignore = "needs a GPU"]
    fn a_partial_reveal_draws_less_than_a_finished_one() {
        let Some(partial) = Rendered::new(&model_with_reply(Some(0.25))) else {
            return;
        };
        let full = Rendered::new(&model_with_reply(None)).expect("full render");
        assert!(
            body_ink(&partial) < body_ink(&full),
            "the reveal drew everything: partial {} vs full {}",
            body_ink(&partial),
            body_ink(&full)
        );
    }

    #[test]
    #[ignore = "needs a GPU"]
    fn a_completed_reveal_matches_the_unanimated_frame() {
        let Some(done) = Rendered::new(&model_with_reply(Some(1.0))) else {
            return;
        };
        let full = Rendered::new(&model_with_reply(None)).expect("full render");
        assert_eq!(
            body_ink(&done),
            body_ink(&full),
            "a finished reveal must land exactly on the static frame"
        );
    }
}
