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

    /// Vertical extent, in logical units, of the composer wash as actually
    /// drawn. Sampled on a column just inside the right edge of the measure
    /// column, where only the well can ink, so prompt glyphs cannot be
    /// mistaken for the well itself.
    fn wash_band(&self) -> Option<(f64, f64)> {
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

fn nodes() -> Vec<(&'static str, Model)> {
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

/// The input box must be *drawn* on the middle of the window, not merely laid
/// out there: this catches a renderer that ignores the centred geometry.
#[test]
#[ignore = "requires a GPU"]
fn the_composer_well_is_drawn_on_the_middle_of_the_window() {
    for (name, model) in nodes() {
        let Some(r) = Rendered::new(&model) else {
            return;
        };
        let f = r.frame;
        let Some((top, bottom)) = r.wash_band() else {
            panic!("{name}: no composer well was drawn");
        };
        assert!(
            (top - f.composer_top).abs() < 2.0 && (bottom - f.composer_bottom).abs() < 2.0,
            "{name}: the drawn well {top:.1}..{bottom:.1} left its geometry {:.1}..{:.1}",
            f.composer_top,
            f.composer_bottom
        );
        let center = (top + bottom) / 2.0;
        assert!(
            (center - f.height / 2.0).abs() < 2.0,
            "{name}: the well centre {center:.1} is not the page middle {:.1}",
            f.height / 2.0
        );
    }
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

/// The top of the page must be bare paper. The app previously carried a
/// wordmark, a status caption, a build-identity row, and a rule up there,
/// which is the clutter this test exists to keep out: any of them reappearing
/// inks the band above the transcript.
#[test]
#[ignore = "requires a GPU"]
fn the_top_of_the_page_is_clear() {
    for (name, model) in nodes() {
        let Some(r) = Rendered::new(&model) else {
            eprintln!("skipping {name}: no GPU");
            return;
        };
        let f = r.frame;
        let darkest = r.darkest_in(0.0, 0.0, f.width - 1.0, f.body_top - 2.0);
        assert!(
            darkest > 0.9,
            "{name}: ink ({darkest:.3} luma) above the transcript"
        );
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
        let above = r.darkest_in(f.left, f.composer_top - 6.0, f.right, f.composer_top - 3.0);
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

/// A model that is not attached must still say so somewhere, or a dead
/// runtime is indistinguishable from an app that ignores input. With the
/// masthead gone, that somewhere is the footnote row.
#[test]
#[ignore = "requires a GPU"]
fn an_unattached_state_still_reports_its_status() {
    let model = crate::states::by_name("connecting").expect("connecting node");
    assert!(
        model.status_footnote().is_some(),
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
    let mut text = TextSystem::default();
    for (name, model) in nodes() {
        let mut scene = Scene::new();
        build_scene(&mut scene, &mut text, &model, (WIDTH, HEIGHT), SCALE);
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

/// The prompt marker must be drawn, and the text must start after it: the
/// marker exists to make the text origin explicit, so overlapping glyphs would
/// defeat it.
#[test]
#[ignore = "requires a GPU"]
fn the_prompt_marker_precedes_the_text() {
    let model = states::by_name("mid_input").expect("node");
    let Some(r) = Rendered::new(&model) else {
        return;
    };
    let f = r.frame;
    let marker = r.darkest_in(
        f.left + crate::layout::COMPOSER_PAD_X,
        f.composer_top + 4.0,
        f.composer_text_left() - 2.0,
        f.composer_bottom - 4.0,
    );
    assert!(marker < 0.8, "no prompt marker was drawn ({marker:.3})");
    // The gap immediately left of the text origin must stay clear, so the
    // marker cannot be touching the message.
    let gap = r.darkest_in(
        f.composer_text_left() - 4.0,
        f.composer_top + 4.0,
        f.composer_text_left() - 1.0,
        f.composer_bottom - 4.0,
    );
    assert!(gap > 0.85, "the marker runs into the text ({gap:.3})");
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
