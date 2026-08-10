//! Frame construction: the pure `Model` -> `Scene` function.
//!
//! Kept separate from the event loop so a frame is a pure function of the
//! model, which is what makes the state-space captures and pixel tests
//! possible.

use crate::text::ParagraphStyle;
use crate::{Model, donut, icons, layout, text};
use vello::Scene;
use vello::kurbo::{Affine, BezPath, Circle, Rect, RoundedRect, Shape};
use vello::peniko::Color;

pub const ATTACHMENT_PREVIEW_ROWS: usize = 4;
const ATTACHMENT_PREVIEW_HEIGHT: f64 = 72.0;
const ATTACHMENT_PREVIEW_GAP: f64 = 8.0;
const ATTACHMENT_CLOSE_SIZE: f64 = 18.0;

pub fn attachment_thumbnail_rects(frame: &layout::Frame, model: &Model) -> Vec<Rect> {
    if model.attachment_previews.is_empty() {
        return Vec::new();
    }
    let prompt_y = frame.composer_top
        + layout::COMPOSER_TEXT_OFFSET
        + ATTACHMENT_PREVIEW_ROWS as f64 * layout::COMPOSER_LINE_HEIGHT;
    let top = frame.composer_top + layout::COMPOSER_TEXT_OFFSET;
    let bottom = (top + ATTACHMENT_PREVIEW_HEIGHT).min(prompt_y - ATTACHMENT_PREVIEW_GAP);
    let mut left = frame.composer_text_left();
    model
        .attachment_previews
        .iter()
        .filter_map(|image| {
            let ratio = image.width as f64 / image.height.max(1) as f64;
            let width = ((bottom - top) * ratio).clamp(36.0, 112.0);
            let rect = Rect::new(left, top, left + width, bottom);
            left += width + ATTACHMENT_PREVIEW_GAP;
            (rect.x1 <= frame.right - layout::COMPOSER_PAD_X).then_some(rect)
        })
        .collect()
}

pub fn attachment_thumbnail_close_rect(rect: Rect) -> Rect {
    Rect::new(
        rect.x1 - ATTACHMENT_CLOSE_SIZE,
        rect.y0,
        rect.x1,
        rect.y0 + ATTACHMENT_CLOSE_SIZE,
    )
}

pub fn attachment_overlay_close_rect(frame: &layout::Frame) -> Rect {
    Rect::new(frame.width - 58.0, 26.0, frame.width - 26.0, 58.0)
}

fn draw_close(scene: &mut Scene, rect: Rect, scale: f64, fill: Color, ink: Color) {
    scene.fill(
        vello::peniko::Fill::NonZero,
        Affine::scale(scale),
        fill,
        None,
        &vello::kurbo::Circle::new(rect.center(), rect.width() * 0.5),
    );
    let inset = rect.width() * 0.31;
    let stroke = vello::kurbo::Stroke::new(1.6);
    for (a, b) in [
        (
            (rect.x0 + inset, rect.y0 + inset),
            (rect.x1 - inset, rect.y1 - inset),
        ),
        (
            (rect.x1 - inset, rect.y0 + inset),
            (rect.x0 + inset, rect.y1 - inset),
        ),
    ] {
        scene.stroke(
            &stroke,
            Affine::scale(scale),
            ink,
            None,
            &vello::kurbo::Line::new(a, b),
        );
    }
}

/// Halftone dot pitch in logical units. Fixed rather than a fraction of the
/// box: a screen is a physical thing, so the dots stay the same size (and so
/// the same optical ink density) whatever size the donut is drawn at, and a
/// smaller donut simply shows fewer of them. Matches the website's hero, which
/// screens a 360px canvas at 76 dots across.
const DOT_PITCH: f64 = 360.0 / 76.0;
// Referenced by `layout::DONUT_MIN_SIDE`'s doc comment: the two together decide
// how few dots a hero may be drawn with.
/// Classic 45-degree halftone screen angle.
const SCREEN_ANGLE: f64 = std::f64::consts::FRAC_PI_4;
/// Dot radius as a fraction of the dot pitch at full luminance.
const DOT_FILL: f64 = 0.62;
/// Luminance below which a dot is not worth drawing.
const DOT_FLOOR: f32 = 0.04;
/// Gamma applied to luminance before sizing a dot.
const DOT_GAMMA: f32 = 0.85;
/// Flattening tolerance for a dot, in logical units. Dots are at most a couple
/// of units across, so a coarse tolerance is invisible and cuts the curve
/// segments (and so the GPU work) well below the exact-circle default.
const CIRCLE_TOLERANCE: f64 = 0.05;

/// Draw the halftone donut into `box_`, sampling `field` as a luminance image.
///
/// The dot lattice is in logical units so the screen density is identical on 1x
/// and HiDPI, exactly like the website's CSS-pixel lattice. Every dot is
/// appended to one `BezPath` and filled in a single draw, which is the same
/// trick the website uses with one canvas path: per-dot fills would mean
/// thousands of separate Vello draw commands per frame.
/// Diameter of the activity spinner's ring, in logical pixels. Sized to a
/// caption line so it reads as part of the text row rather than as a graphic
/// bolted next to it.
pub(crate) const SPINNER_SIZE: f64 = 13.0;

/// The activity spinner: a ring of halftone dots with a bright head that walks
/// around it. Same visual language as the hero donut, so "the agent is working"
/// looks like part of the app rather than a stock throbber.
pub(crate) fn draw_spinner(
    scene: &mut Scene,
    activity: &crate::activity::Activity,
    center: (f64, f64),
    ink: Color,
    scale: f64,
    now: std::time::Instant,
) {
    let lead = activity.frame(now);
    let count = crate::activity::SPINNER_DOTS;
    let radius = SPINNER_SIZE / 2.0;
    for index in 0..count {
        // Distance behind the head, so the ring reads as a comet trail and the
        // direction of motion is unambiguous even in a still frame.
        let behind = (count + lead - index) % count;
        let fade = 1.0 - (behind as f32 / count as f32);
        let angle =
            std::f64::consts::TAU * index as f64 / count as f64 - std::f64::consts::FRAC_PI_2;
        let dot = Circle::new(
            (
                center.0 + radius * angle.cos(),
                center.1 + radius * angle.sin(),
            ),
            // The head is a full dot and the tail shrinks, so the motion is
            // carried by size as well as by alpha: alpha alone disappears on a
            // faint caption colour.
            (1.0 + 1.4 * f64::from(fade)) * 0.62,
        );
        scene.fill(
            vello::peniko::Fill::NonZero,
            Affine::scale(scale),
            ink.with_alpha(0.25 + 0.75 * fade),
            None,
            &dot,
        );
    }
}

/// A background task's progress bar: a faint track with a bright fill.
///
/// Two modes, because a task either knows how far along it is or does not, and
/// pretending otherwise is the one thing a progress indicator must not do. A
/// reported percentage fills the track from the left; a task that can only say
/// "still working" gets a segment that sweeps across it, so the bar stays
/// honest about what is known while still proving the wait is alive.
pub(crate) fn draw_progress_bar(
    scene: &mut Scene,
    track: Rect,
    fraction: Option<f64>,
    theme: &crate::theme::Theme,
    scale: f64,
    elapsed: std::time::Duration,
) {
    let radius = crate::transcript::PROGRESS_BAR_RADIUS;
    scene.fill(
        vello::peniko::Fill::NonZero,
        Affine::scale(scale),
        theme.wash,
        None,
        &RoundedRect::from_rect(track, radius),
    );
    let width = track.width().max(0.0);
    if width <= 0.0 {
        return;
    }
    let (start, end) = match fraction {
        Some(fraction) => (0.0, width * fraction.clamp(0.0, 1.0)),
        None => {
            let sweep = crate::transcript::PROGRESS_SWEEP_FRACTION * width;
            let period = crate::transcript::PROGRESS_SWEEP_PERIOD.as_secs_f64();
            // A bounce rather than a wrap: the segment eases from one end of
            // the track to the other and back, so it is fully visible at every
            // phase (including zero, which is what a still capture draws) and
            // never pops out of existence at the edges.
            let phase = (elapsed.as_secs_f64() % period) / period;
            let travel = (1.0 - (std::f64::consts::TAU * phase).cos()) / 2.0;
            let start = travel * (width - sweep).max(0.0);
            (start, (start + sweep).min(width))
        }
    };
    if end <= start {
        return;
    }
    scene.fill(
        vello::peniko::Fill::NonZero,
        Affine::scale(scale),
        theme.muted,
        None,
        &RoundedRect::from_rect(
            Rect::new(track.x0 + start, track.y0, track.x0 + end, track.y1),
            radius,
        ),
    );
}

/// `grow` scales the whole halftone screen about the box's centre, which is how
/// the boot reveal brings the donut in: scaling the *box* instead would keep the
/// fixed dot pitch and simply show fewer dots, i.e. a coarsening blob rather
/// than a donut arriving.
fn draw_donut(
    scene: &mut Scene,
    field: &donut::Donut,
    box_: Rect,
    ink: Color,
    scale: f64,
    grow: f64,
) {
    if grow <= 0.0 || ink.components[3] <= 0.0 {
        return;
    }
    let side = box_.width().min(box_.height());
    if side < layout::DONUT_MIN_SIDE {
        return;
    }
    let pitch = DOT_PITCH;
    let cells = side / pitch;
    let (sin_a, cos_a) = SCREEN_ANGLE.sin_cos();
    let cx = box_.x0 + box_.width() / 2.0;
    let cy = box_.y0 + box_.height() / 2.0;
    let (x0, y0) = (cx - side / 2.0, cy - side / 2.0);
    // A rotated lattice must cover the square, so extend it by sqrt(2).
    let ext = (cells * std::f64::consts::FRAC_1_SQRT_2).ceil() as i32 + 1;
    let grid = field.grid() as f32;
    let per_unit = grid / side as f32;

    let mut dots = BezPath::new();
    for j in -ext..=ext {
        for i in -ext..=ext {
            let px = cx + (i as f64 * cos_a - j as f64 * sin_a) * pitch;
            let py = cy + (i as f64 * sin_a + j as f64 * cos_a) * pitch;
            // Reject dots outside the donut's square before sampling: on a
            // 45-degree lattice that is ~1/3 of the candidates.
            if px < x0 || px > x0 + side || py < y0 || py > y0 + side {
                continue;
            }
            let lum = field.sample((px - x0) as f32 * per_unit, (py - y0) as f32 * per_unit);
            if lum <= DOT_FLOOR {
                continue;
            }
            let radius = f64::from(lum.powf(DOT_GAMMA)) * pitch * DOT_FILL;
            dots.extend(Circle::new((px, py), radius).path_elements(CIRCLE_TOLERANCE));
        }
    }
    let centre = vello::kurbo::Vec2::new(cx, cy);
    let transform = Affine::scale(scale)
        * Affine::translate(centre)
        * Affine::scale(grow)
        * Affine::translate(-centre);
    scene.fill(vello::peniko::Fill::NonZero, transform, ink, None, &dots);
}

/// Draw the hero's text: the wordmark over the donut and the one line of
/// invitation under it, stacked and centred exactly like the website's landing
/// section.
///
/// The donut itself is drawn by the caller, outside the chrome reveal layer:
/// during boot the torus arrives *first* and the words after it, so the two
/// cannot share one draw.
fn draw_hero_text(
    scene: &mut Scene,
    text: &mut text::TextSystem,
    model: &Model,
    hero: layout::Hero,
    frame: &layout::Frame,
    scale: f64,
) {
    let column = frame.column() as f32;
    // The wordmark: the same "jcode" that sits above the donut on the website.
    text.draw_paragraph_scaled(
        scene,
        "jcode",
        (frame.left, hero.wordmark_top),
        column,
        ParagraphStyle {
            font_size: layout::HERO_WORDMARK_SIZE,
            color: model.theme.text,
            align: text::Align::Center,
            letter_spacing_em: -0.02,
            line_height: layout::HERO_LINE_HEIGHT,
            ..Default::default()
        },
        scale,
    );
    text.draw_paragraph_scaled(
        scene,
        HERO_TAGLINE,
        (frame.left, hero.tagline_top),
        column,
        ParagraphStyle {
            font_size: layout::HERO_TAGLINE_SIZE,
            color: model.theme.muted,
            align: text::Align::Center,
            line_height: layout::HERO_LINE_HEIGHT,
            ..Default::default()
        },
        scale,
    );
}

/// Draw the session strip: a row of small rectangles at the top of the window,
/// one per live session, enclosed in an outlined rectangle per working
/// directory.
///
/// Deliberately the same visual language as the author's waybar
/// `niri-workspaces` module, because it is the language he already reads
/// without thinking: thin ticks for the sessions in a place, with the focused
/// one a wide solid block.
///
/// Purely geometric: the group's name used to be spelled out beside its bars,
/// which turned the chrome row into a line of prose that had to be *read*
/// (`jcode-website jcode jcode-desktop2 ...`) and grew with every checkout.
/// The enclosure carries the same information as shape, so the row is scanned
/// rather than read, and stays the same width whatever the directories are
/// called. Which place is which is answered by the overview, where there is
/// room to name them.
fn draw_strip(
    scene: &mut Scene,
    text: &mut text::TextSystem,
    model: &Model,
    band: (f64, f64),
    frame: &layout::Frame,
    scale: f64,
) {
    let (top, bottom) = band;
    let items = crate::strip::layout_items(&model.strips, frame.left, frame.right);

    // Blocks are centred in the band; the enclosure adds its padding around
    // them, so both are derived from the same centre line.
    let block_top = top + (bottom - top - layout::STRIP_BAR_HEIGHT) / 2.0;
    let block_bottom = block_top + layout::STRIP_BAR_HEIGHT;
    let pad = layout::STRIP_FRAME_PAD;

    for item in items {
        match item {
            crate::strip::Item::Strip {
                x,
                width,
                focused,
                strip: _,
            } => {
                // The enclosure is a hairline so it frames without competing
                // with the blocks inside it. The focused group's outline is
                // full-weight rule ink; the others are faint, so the row shows
                // where you are before you count anything.
                let color = if focused {
                    model.theme.muted
                } else {
                    model.theme.rule
                };
                scene.stroke(
                    &vello::kurbo::Stroke::new(frame.hairline().max(1.0 / scale)),
                    Affine::scale(scale),
                    color,
                    None,
                    &RoundedRect::new(
                        x,
                        block_top - pad,
                        x + width,
                        block_bottom + pad,
                        layout::STRIP_FRAME_RADIUS,
                    ),
                );
            }
            crate::strip::Item::Panel {
                x,
                width,
                focused,
                strip,
                panel,
            } => {
                // Unfocused blocks are dim so the focused one reads instantly;
                // a busy session is drawn at full ink even when unfocused, so
                // work happening off-screen is visible rather than silent.
                let busy = model
                    .strips
                    .strips()
                    .get(strip)
                    .and_then(|strip| strip.panels.get(panel))
                    .map(|entry| entry.busy)
                    .unwrap_or(false);
                let color = if focused {
                    model.theme.text
                } else if busy {
                    model.theme.muted
                } else {
                    model.theme.rule
                };
                scene.fill(
                    vello::peniko::Fill::NonZero,
                    Affine::scale(scale),
                    color,
                    None,
                    &RoundedRect::new(x, block_top, x + width, block_bottom, 1.0),
                );
            }
        }
    }

    // The conversation earns a heading only once it has begun. Until the
    // asynchronous title generator catches up, identify it by its ordinal in
    // the same stable session order used by the strip.
    if model.transcript.has_user_message() {
        let heading = model
            .strips
            .focused_title()
            .map(str::to_string)
            .or_else(|| model.transcript.provisional_heading())
            .or_else(|| model.strips.focused_heading())
            .unwrap_or_else(|| "1st chat".to_string());
        text.draw_paragraph_scaled(
            scene,
            &heading,
            (frame.left, top),
            frame.column() as f32,
            ParagraphStyle {
                font_size: layout::CAPTION_SIZE,
                color: model.theme.muted,
                align: text::Align::Center,
                line_height: 1.0,
                ..Default::default()
            },
            scale,
        );
    }
}

/// The tagline under the donut, matching the website's hero copy.
const HERO_TAGLINE: &str = "an open source coding agent, written in rust";

/// Draw the settings panel the gear opens: one row per setting, each a label
/// on the left and its current value on the right.
///
/// Values rather than checkboxes or switches, because every setting here has
/// more than two states and a row that says `theme   dark` answers "what is it
/// now" and "what will clicking do" in the same three words.
fn draw_settings_panel(
    scene: &mut Scene,
    text: &mut text::TextSystem,
    model: &Model,
    frame: &layout::Frame,
    scale: f64,
) {
    let theme = &model.theme;
    let rows = model.panel.rows();
    let panel = frame.panel(rows.len());
    scene.fill(
        vello::peniko::Fill::NonZero,
        Affine::scale(scale),
        theme.field,
        None,
        &RoundedRect::from_rect(panel, layout::PANEL_RADIUS),
    );
    scene.stroke(
        &vello::kurbo::Stroke::new(layout::COMPOSER_BORDER),
        Affine::scale(scale),
        theme.field_border,
        None,
        &RoundedRect::from_rect(panel, layout::PANEL_RADIUS),
    );
    for (index, row) in rows.iter().enumerate() {
        let band = frame.panel_row(rows.len(), index);
        if model.panel.hover() == Some(index) {
            scene.fill(
                vello::peniko::Fill::NonZero,
                Affine::scale(scale),
                theme.wash,
                None,
                &RoundedRect::from_rect(band, layout::PANEL_RADIUS / 2.0),
            );
        }
        // Both captions sit on one baseline inside the row, so the label and
        // its value read as one sentence rather than as two columns.
        let baseline = band.y0 + (band.height() - f64::from(layout::CAPTION_SIZE) * 1.4) / 2.0;
        let width = (band.width() - layout::PANEL_TEXT_PAD * 2.0).max(1.0) as f32;
        let left = band.x0 + layout::PANEL_TEXT_PAD;
        text.draw_paragraph_scaled(
            scene,
            row.label(),
            (left, baseline),
            width,
            ParagraphStyle {
                font_size: layout::CAPTION_SIZE,
                color: theme.muted,
                letter_spacing_em: 0.1,
                ..Default::default()
            },
            scale,
        );
        text.draw_paragraph_scaled(
            scene,
            model.settings.value(*row),
            (left, baseline),
            width,
            ParagraphStyle {
                font_size: layout::CAPTION_SIZE,
                color: theme.text,
                letter_spacing_em: 0.1,
                align: text::Align::End,
                ..Default::default()
            },
            scale,
        );
    }
}

/// Draw the catalog as a temporary object in the middle of the transcript.
/// Its reveal is clipped from the centre while the transcript parts around it,
/// so it reads as space being made rather than a menu covering conversation.
fn draw_model_picker(
    scene: &mut Scene,
    text: &mut text::TextSystem,
    model: &Model,
    frame: &layout::Frame,
    scale: f64,
) {
    let theme = &model.theme;
    if !model.model_picker.is_visible() {
        return;
    }
    let rows = model.model_picker.visual_rows();
    let menu = frame.model_menu(rows);
    let phase = model.model_picker.phase();
    let centre = (menu.y0 + menu.y1) / 2.0;
    let reveal = Rect::new(
        menu.x0,
        centre - menu.height() * phase / 2.0,
        menu.x1,
        centre + menu.height() * phase / 2.0,
    );
    scene.push_clip_layer(vello::peniko::Fill::NonZero, Affine::scale(scale), &reveal);
    scene.fill(
        vello::peniko::Fill::NonZero,
        Affine::scale(scale),
        theme.background,
        None,
        &RoundedRect::from_rect(menu, layout::MODEL_MENU_RADIUS),
    );
    scene.stroke(
        &vello::kurbo::Stroke::new(layout::COMPOSER_BORDER),
        Affine::scale(scale),
        theme.rule,
        None,
        &RoundedRect::from_rect(menu, layout::MODEL_MENU_RADIUS),
    );
    let labels = model.model_picker.row_labels();
    for (index, label) in labels.iter().enumerate() {
        let band = frame.model_menu_row(rows, index);
        if model.model_picker.hover() == Some(index) {
            scene.fill(
                vello::peniko::Fill::NonZero,
                Affine::scale(scale),
                theme.wash,
                None,
                &RoundedRect::from_rect(band, layout::MODEL_MENU_RADIUS / 2.0),
            );
        }
        let current = if model.model_picker.stage() == crate::model_picker::Stage::Model {
            model
                .model_picker
                .visible_models()
                .get(index)
                .is_some_and(|value| model.model_picker.current() == Some(*value))
        } else {
            false
        };
        let left = band.x0 + layout::MODEL_MENU_TEXT_PAD;
        let baseline = band.y0 + (band.height() - f64::from(layout::CAPTION_SIZE) * 1.4) / 2.0;
        text.draw_paragraph_scaled(
            scene,
            label,
            (left, baseline),
            (band.width() - layout::MODEL_MENU_TEXT_PAD * 2.0).max(1.0) as f32,
            ParagraphStyle {
                font_size: layout::CAPTION_SIZE,
                color: if current { theme.text } else { theme.muted },
                letter_spacing_em: 0.05,
                ..Default::default()
            },
            scale,
        );
        if current {
            scene.fill(
                vello::peniko::Fill::NonZero,
                Affine::scale(scale),
                theme.text,
                None,
                &Circle::new((band.x1 - 8.0, band.y0 + band.height() / 2.0), 2.0),
            );
        }
    }
    scene.pop_layer();
}

/// Body paragraph style for transcript prose. One definition, so measuring in
/// [`crate::viewport`] and drawing here can never disagree.
pub fn transcript_body_style(model: &Model) -> ParagraphStyle {
    ParagraphStyle {
        font_size: layout::BODY_SIZE,
        color: model.theme.text,
        line_height: layout::BODY_LEADING as f32,
        ..Default::default()
    }
}

/// Width of the scrollbar's thumb, in logical pixels. A hairline-ish sliver:
/// this is a position readout, not a drag handle competing with the text.
const SCROLLBAR_WIDTH: f64 = 3.0;
/// Gap between the text column's right edge and the bar.
const SCROLLBAR_GAP: f64 = 6.0;
/// Shortest the thumb may be drawn. Proportional sizing alone makes a very
/// long conversation's thumb a dot, which stops reading as a position.
const SCROLLBAR_MIN_THUMB: f64 = 24.0;

/// Draw the transcript scrollbar: a thumb whose length is the visible
/// fraction of the conversation and whose position is where you are in it.
///
/// It is only drawn while [`crate::scroll::Smooth`] says it is lit, so it
/// appears when you scroll and fades out afterwards rather than sitting on the
/// page permanently. Drawn outside the transcript's clip so it can hug the
/// region's edge, and skipped entirely when everything already fits.
fn draw_scrollbar(
    scene: &mut Scene,
    text: &mut text::TextSystem,
    cache: &mut crate::paint::TranscriptCache,
    model: &Model,
    frame: &layout::Frame,
    scale: f64,
) {
    let alpha = model.smooth.alpha() as f32;
    if alpha <= 0.0 {
        return;
    }
    let region_height = (frame.body_bottom - frame.body_top).max(0.0);
    if region_height <= 0.0 {
        return;
    }
    let width = (frame.column() - crate::transcript::USER_PAD_X * 2.0).max(1.0);
    let laid = cache.lay_out(
        text,
        &model.transcript,
        width,
        &model.theme,
        transcript_body_style(model),
        scale,
    );
    let view = crate::viewport::Viewport::new(laid, region_height, model.view_scroll());
    let max = view.max_scroll();
    // Nothing to scroll: a full-height thumb would just be a border.
    if max <= 0.5 {
        return;
    }
    let content = view.content_height.max(1.0);
    let thumb =
        (region_height / content * region_height).max(SCROLLBAR_MIN_THUMB.min(region_height));
    // scroll counts pixels *up from the tail*, so 0 puts the thumb at the
    // bottom, which is where the newest message is.
    let travel = (region_height - thumb).max(0.0);
    let from_tail = (model.view_scroll().clamp(0.0, max)) / max;
    let top = frame.body_top + travel * (1.0 - from_tail);
    let left = frame.right + SCROLLBAR_GAP;
    let color = model.theme.rule.multiply_alpha(alpha);
    scene.fill(
        vello::peniko::Fill::NonZero,
        Affine::scale(scale),
        color,
        None,
        &RoundedRect::new(
            left,
            top,
            left + SCROLLBAR_WIDTH,
            top + thumb,
            SCROLLBAR_WIDTH / 2.0,
        ),
    );
}

/// Vertical paint bounds for a diff band.
///
/// Whole-row washes deliberately overlap by one physical pixel in total. Vello
/// rasterizes each rectangle independently, so merely sharing a floating-point
/// edge is not enough to guarantee that the device pixel at that edge is
/// covered. Keeping this calculation separate gives the rule a cheap,
/// GPU-independent regression test.
fn diff_band_y(rect: vello::kurbo::Rect, origin: f64, hairline: f64, emphasis: bool) -> (f64, f64) {
    let bleed = if emphasis { 0.0 } else { hairline * 0.5 };
    (origin + rect.y0 - bleed, origin + rect.y1 + bleed)
}

/// Draw the conversation.
///
/// Roles are distinguished structurally rather than by a marker glyph: your
/// message is a tinted card with the composer's own corner radius, so it reads
/// as the thing you typed, and the reply is plain ink on paper. That is why
/// there is no `>` here; a prompt marker was standing in for structure the
/// model did not have.
fn draw_transcript(
    scene: &mut Scene,
    text: &mut text::TextSystem,
    cache: &mut crate::paint::TranscriptCache,
    model: &Model,
    frame: &layout::Frame,
    scale: f64,
) {
    use crate::transcript::{CODE_PAD_Y, Role, USER_PAD_X, USER_RADIUS};
    /// Width of the plan card's header gauge. Short on purpose: it is a
    /// glanceable fraction beside the words that state it, not a download bar.
    const TODO_GAUGE_WIDTH: f64 = 72.0;
    /// Centre of a task's state dot, right of the item's text edge. Sits in
    /// the column markdown's own `• ` marker occupied, so the text never
    /// moves when the dot replaces the bullet.
    const TODO_DOT_X: f64 = 7.0;
    /// Radius of a task's state dot.
    const TODO_DOT_RADIUS: f64 = 6.0;
    /// Air between the stepper rail's ends and the dots it links.
    const TODO_RAIL_GAP: f64 = 3.0;
    use jcode_render_core::BlockKind;

    let theme = &model.theme;
    let region_height = (frame.body_bottom - frame.body_top).max(0.0);
    // A user card is inset by its own padding, so both roles wrap to the same
    // text column and the conversation keeps one measure.
    let width = (frame.column() - USER_PAD_X * 2.0).max(1.0);
    let laid = cache.lay_out(
        text,
        &model.transcript,
        width,
        theme,
        transcript_body_style(model),
        scale,
    );
    // The glide holds the view slightly above the tail while the conversation
    // grows, so a new line slides in instead of snapping the page up by a line
    // height. It decays to zero, so this cannot drift the scroll position.
    let view = crate::viewport::Viewport::new(laid, region_height, model.view_scroll());

    // Only the trailing text message is being revealed; everything above it
    // has been read and must be drawn whole. The live tool card is skipped:
    // it is pinned to the tail as a status readout and appears whole, while
    // the reveal animates the text arriving above it (the same message
    // `Transcript::streaming_len` counts, or the two would disagree). Queued
    // messages are skipped the same way: they sit *below* the streaming text,
    // waiting for their turn, and were typed rather than streamed.
    let streaming_index = laid
        .iter()
        .rposition(|message| {
            !matches!(
                message.role,
                Role::Tool | Role::Notice | Role::Edit | Role::Progress | Role::Todo
            ) && message.delivery != Some(crate::ack::Delivery::Queued)
        })
        .filter(|_| model.stream.is_revealing())
        .filter(|index| laid[*index].role != Role::User);

    let now = std::time::Instant::now();
    for placed in &view.visible {
        let mut message_top = frame.body_top + placed.top;
        if model.model_picker.is_visible() {
            let menu = frame.model_menu(model.model_picker.visual_rows());
            message_top += model.model_picker.transcript_shift(
                region_height,
                menu.height(),
                placed.top,
                placed.message.height,
            );
        }
        let is_user = placed.message.role == Role::User;
        // The acknowledgement nod. Applied to the card *and* its text, so the
        // message moves as one object; it decays to zero, so nothing here can
        // leave the transcript permanently off its column.
        let wiggle = placed
            .message
            .delivery
            .map_or(0.0, |delivery| delivery.wiggle(now));
        // The delivery tone: a message the agent has not confirmed yet is
        // drawn faint, and the acknowledgement ramps it to full ink over the
        // wiggle. One layer over the whole card, so the wash and the text
        // fade as one object rather than as two.
        let tone = placed
            .message
            .delivery
            .map_or(1.0, |delivery| delivery.tone(now));
        let toned = tone < 1.0;
        if toned {
            scene.push_layer(
                vello::peniko::Fill::NonZero,
                vello::peniko::Mix::Normal,
                tone as f32,
                Affine::scale(scale),
                &Rect::new(
                    frame.left + wiggle - crate::ack::WIGGLE_AMPLITUDE,
                    message_top,
                    frame.right + wiggle + crate::ack::WIGGLE_AMPLITUDE,
                    message_top + placed.message.height,
                ),
            );
        }
        // The user's card: the same fill and radius as the composer, so the
        // message and the field it came from are visibly one object.
        if is_user {
            if placed.sticky {
                // The reply continues to scroll at its natural position behind
                // the pinned prompt. Clear the prompt's full band first so text
                // cannot show through its rounded corners or the breathing room
                // below the card.
                scene.fill(
                    vello::peniko::Fill::NonZero,
                    Affine::scale(scale),
                    theme.background,
                    None,
                    &Rect::new(
                        frame.left,
                        message_top,
                        frame.right,
                        message_top + placed.message.height + crate::transcript::MESSAGE_GAP,
                    ),
                );
            }
            scene.fill(
                vello::peniko::Fill::NonZero,
                Affine::scale(scale),
                theme.wash,
                None,
                &RoundedRect::new(
                    frame.left + wiggle,
                    message_top,
                    frame.right + wiggle,
                    message_top + placed.message.height,
                    USER_RADIUS,
                ),
            );
        }
        let text_left = frame.left + USER_PAD_X + wiggle;
        let text_top = message_top + placed.message.top_padding();

        // A thought carries no rule and no indent: it is set apart by being
        // dimmer and slightly smaller than the reply (see `lay_out_message`).
        // Furniture down the left edge made every aside look like a quoted
        // block, which is louder than a thought should ever read.

        // The live tool card: one card for the call running right now, on
        // the composer's wash with the app's halftone spinner beside its
        // label. There is at most one of these in the transcript, so a busy
        // turn reads as a single line of "what is being done" rather than a
        // growing log of finished calls.
        if placed.message.role == Role::Tool {
            scene.fill(
                vello::peniko::Fill::NonZero,
                Affine::scale(scale),
                theme.wash,
                None,
                &RoundedRect::new(
                    frame.left,
                    message_top,
                    frame.right,
                    message_top + placed.message.height,
                    USER_RADIUS,
                ),
            );
            // The spinner only turns while the turn runs: a card drawn in a
            // still capture (or after an interrupt raced the clear) must not
            // claim live work.
            if model.busy {
                draw_spinner(
                    scene,
                    &model.activity,
                    (
                        text_left + SPINNER_SIZE / 2.0,
                        message_top + placed.message.height / 2.0,
                    ),
                    theme.muted,
                    scale,
                    std::time::Instant::now(),
                );
            }
        }

        // A background task's progress card: the same wash as the live tool
        // card (both are live status, not conversation) with a bar under its
        // label. The bar is drawn rather than written as text because a
        // fraction the eye reads at a glance is the whole point of waiting on
        // a long task, and `50% · linking` on its own is a sentence to parse.
        if placed.message.role == Role::Progress {
            scene.fill(
                vello::peniko::Fill::NonZero,
                Affine::scale(scale),
                theme.wash,
                None,
                &RoundedRect::new(
                    frame.left,
                    message_top,
                    frame.right,
                    message_top + placed.message.height,
                    USER_RADIUS,
                ),
            );
            let bar_top = message_top + placed.message.height
                - crate::transcript::TOOL_PAD_Y
                - crate::transcript::PROGRESS_BAR_HEIGHT;
            draw_progress_bar(
                scene,
                Rect::new(
                    // Aligned with the card's label, which is indented by
                    // `TOOL_INSET`: a bar starting left of the text it belongs
                    // to reads as furniture rather than as that task's readout.
                    text_left + crate::transcript::TOOL_INSET,
                    bar_top,
                    frame.right - USER_PAD_X,
                    bar_top + crate::transcript::PROGRESS_BAR_HEIGHT,
                ),
                placed.message.fraction(),
                theme,
                scale,
                // The cards' shared clock drives the indeterminate sweep, so
                // every bar sweeps in step and a still capture (no clock) draws
                // the segment at its start rather than at a random phase.
                model
                    .progress_clock
                    .map(|started| now.saturating_duration_since(started))
                    .unwrap_or_default(),
            );
        }

        // A plan snapshot is history rather than live activity. It gets a quiet
        // card with a document edge, and its completion is a compact gauge on
        // the header line: the header already says "2 of 5 tasks" in words, so
        // the bar sits beside the words it illustrates instead of underlining
        // the whole card like a download.
        if placed.message.role == Role::Todo {
            scene.fill(
                vello::peniko::Fill::NonZero,
                Affine::scale(scale),
                theme.wash,
                None,
                &RoundedRect::new(
                    frame.left,
                    message_top,
                    frame.right,
                    message_top + placed.message.height,
                    USER_RADIUS,
                ),
            );
            // A hairline gives this durable snapshot a document edge. Live
            // activity cards intentionally have no border, which makes the
            // plan read as something retained rather than another spinner.
            scene.stroke(
                &vello::kurbo::Stroke::new(frame.hairline()),
                Affine::scale(scale),
                theme.rule,
                None,
                &RoundedRect::new(
                    frame.left + frame.hairline() / 2.0,
                    message_top + frame.hairline() / 2.0,
                    frame.right - frame.hairline() / 2.0,
                    message_top + placed.message.height - frame.hairline() / 2.0,
                    USER_RADIUS,
                ),
            );
            // The gauge, right-aligned on the header's first line. Centred on
            // the line's own middle so the words and the bar share a baseline
            // band rather than the bar floating between rows. Drawn here
            // rather than via `draw_progress_bar`, whose track is the page
            // wash and would vanish against the card's identical wash.
            let line = f64::from(layout::BODY_SIZE) * layout::BODY_LEADING;
            let gauge_mid = message_top + placed.message.top_padding() + line / 2.0;
            let gauge_right = frame.right - USER_PAD_X;
            let track = Rect::new(
                gauge_right - TODO_GAUGE_WIDTH,
                gauge_mid - crate::transcript::PROGRESS_BAR_HEIGHT / 2.0,
                gauge_right,
                gauge_mid + crate::transcript::PROGRESS_BAR_HEIGHT / 2.0,
            );
            scene.fill(
                vello::peniko::Fill::NonZero,
                Affine::scale(scale),
                theme.code_wash,
                None,
                &RoundedRect::from_rect(track, crate::transcript::PROGRESS_BAR_RADIUS),
            );
            let fraction = placed.message.fraction().unwrap_or(0.0).clamp(0.0, 1.0);
            if fraction > 0.0 {
                scene.fill(
                    vello::peniko::Fill::NonZero,
                    Affine::scale(scale),
                    theme.muted,
                    None,
                    &RoundedRect::from_rect(
                        Rect::new(
                            track.x0,
                            track.y0,
                            track.x0 + track.width() * fraction,
                            track.y1,
                        ),
                        crate::transcript::PROGRESS_BAR_RADIUS,
                    ),
                );
            }
        }

        // An edit card carries no furniture down its left edge. The diff body
        // is already the loudest object on the page (a wash, per-row bands,
        // and hue), and a rule beside it only narrowed the measure while
        // saying a second time what the colour had already said.

        // A failure notice is a red card. The translucent fill keeps the error
        // text legible in both themes while making the whole message, rather
        // than a line of furniture beside it, carry the error state.
        if placed.message.role == Role::Notice {
            scene.fill(
                vello::peniko::Fill::NonZero,
                Affine::scale(scale),
                theme.error.with_alpha(0.14),
                None,
                &RoundedRect::new(
                    frame.left,
                    message_top,
                    frame.right,
                    message_top + placed.message.height,
                    USER_RADIUS,
                ),
            );
        }

        // Glyphs in this message, and how many earlier blocks have consumed,
        // so the reveal sweeps across block boundaries as one motion. Counts
        // come from layout time, so this is arithmetic, not a per-frame walk
        // over every glyph run.
        let message_glyphs: usize = match streaming_index {
            Some(index) if index == placed.index => {
                placed.message.blocks.iter().map(|block| block.glyphs).sum()
            }
            _ => 0,
        };
        let mut drawn_glyphs = 0usize;

        let mut todo_index = 0usize;
        // The plan card's stepper rail: a faint line linking consecutive tasks
        // of one group, drawn before the blocks so every dot sits on top of
        // it. The rail is what makes the checklist read as one route through
        // the work rather than as five disconnected bullets; it breaks at a
        // group label because the label already breaks the sequence.
        if placed.message.role == Role::Todo {
            let line = f64::from(layout::BODY_SIZE) * layout::BODY_LEADING;
            let mut previous: Option<(usize, f64, f64)> = None;
            for (block_index, block) in placed.message.blocks.iter().enumerate() {
                if !matches!(block.kind, BlockKind::ListItem { .. }) {
                    continue;
                }
                let x = text_left + block.edge() + TODO_DOT_X;
                let y = text_top + block.top + line / 2.0;
                if let Some((prev_index, prev_x, prev_y)) = previous
                    && prev_index + 1 == block_index
                    && (prev_x - x).abs() < 0.5
                {
                    scene.fill(
                        vello::peniko::Fill::NonZero,
                        Affine::scale(scale),
                        theme.rule,
                        None,
                        &Rect::new(
                            x - frame.hairline() / 2.0,
                            prev_y + TODO_DOT_RADIUS + TODO_RAIL_GAP,
                            x + frame.hairline() / 2.0,
                            y - TODO_DOT_RADIUS - TODO_RAIL_GAP,
                        ),
                    );
                }
                previous = Some((block_index, x, y));
            }
        }
        for (block_index, block) in placed.message.blocks.iter().enumerate() {
            let block_top = text_top + block.top;
            // The block's own left edge: inside any list indent it inherited,
            // so a fenced block written under an item keeps its wash under that
            // item instead of back at the margin.
            let block_left = text_left + block.edge();
            if placed.message.role == Role::Todo && matches!(block.kind, BlockKind::ListItem { .. })
            {
                use crate::todos::TodoState;
                let state = placed
                    .message
                    .todo_states
                    .get(todo_index)
                    .copied()
                    .unwrap_or(TodoState::Pending);
                todo_index += 1;

                // The active task gets a full-width row highlight. State is
                // spatial and graphical here, not text pretending to be UI.
                if state == TodoState::Active {
                    scene.fill(
                        vello::peniko::Fill::NonZero,
                        Affine::scale(scale),
                        theme.code_wash,
                        None,
                        &RoundedRect::new(
                            frame.left + USER_PAD_X / 2.0,
                            block_top - 3.0,
                            frame.right - USER_PAD_X / 2.0,
                            block_top + block.height + 3.0,
                            USER_RADIUS,
                        ),
                    );
                }

                // Paint over markdown's list marker, then draw a precise native
                // control in its place. The task text remains normal selectable
                // transcript text, while the state belongs to the scene. The
                // dot is centred on the item's first text line, so a wrapped
                // task keeps its control beside the words it belongs to.
                let line = f64::from(layout::BODY_SIZE) * layout::BODY_LEADING;
                let center = (block_left + TODO_DOT_X, block_top + line / 2.0);
                // Clear the glyph marker and a dot's worth of rail behind the
                // control, in the row's own surface colour.
                scene.fill(
                    vello::peniko::Fill::NonZero,
                    Affine::scale(scale),
                    if state == TodoState::Active {
                        theme.code_wash
                    } else {
                        theme.wash
                    },
                    None,
                    &Circle::new(center, TODO_DOT_RADIUS + 3.0),
                );
                match state {
                    // Yet to do: an empty ring, the unfilled hole in the route.
                    TodoState::Pending => scene.stroke(
                        &vello::kurbo::Stroke::new(1.4),
                        Affine::scale(scale),
                        theme.faint,
                        None,
                        &Circle::new(center, TODO_DOT_RADIUS - 0.7),
                    ),
                    // In progress: a bullseye in full ink, the loudest of the
                    // three states because it marks where the work *is*.
                    TodoState::Active => {
                        scene.stroke(
                            &vello::kurbo::Stroke::new(1.4),
                            Affine::scale(scale),
                            theme.text,
                            None,
                            &Circle::new(center, TODO_DOT_RADIUS - 0.7),
                        );
                        scene.fill(
                            vello::peniko::Fill::NonZero,
                            Affine::scale(scale),
                            theme.text,
                            None,
                            &Circle::new(center, TODO_DOT_RADIUS - 3.2),
                        );
                    }
                    // Done: a filled disc with a check cut out of it. Muted,
                    // because finished work is context rather than news.
                    TodoState::Completed => {
                        scene.fill(
                            vello::peniko::Fill::NonZero,
                            Affine::scale(scale),
                            theme.muted,
                            None,
                            &Circle::new(center, TODO_DOT_RADIUS),
                        );
                        let mut check = BezPath::new();
                        check.move_to((center.0 - 2.7, center.1 + 0.1));
                        check.line_to((center.0 - 0.7, center.1 + 2.1));
                        check.line_to((center.0 + 3.0, center.1 - 2.2));
                        scene.stroke(
                            &vello::kurbo::Stroke::new(1.5)
                                .with_caps(vello::kurbo::Cap::Round)
                                .with_join(vello::kurbo::Join::Round),
                            Affine::scale(scale),
                            theme.wash,
                            None,
                            &check,
                        );
                    }
                }
            }
            match &block.kind {
                // A code block gets a wash and an inset, so it reads as a
                // quoted artefact rather than as more prose.
                BlockKind::CodeBlock { .. } => {
                    scene.fill(
                        vello::peniko::Fill::NonZero,
                        Affine::scale(scale),
                        theme.wash,
                        None,
                        &RoundedRect::new(
                            block_left,
                            block_top,
                            frame.right - USER_PAD_X,
                            block_top + block.height,
                            layout::COMPOSER_RADIUS,
                        ),
                    );
                }
                // A quote gets a rule down its left edge, the print
                // convention, instead of a repeated `>` on every line.
                BlockKind::BlockQuote => {
                    scene.fill(
                        vello::peniko::Fill::NonZero,
                        Affine::scale(scale),
                        theme.rule,
                        None,
                        &Rect::new(
                            block_left,
                            block_top,
                            block_left + frame.hairline() * 2.0,
                            block_top + block.height,
                        ),
                    );
                }
                BlockKind::ThematicBreak => {
                    scene.fill(
                        vello::peniko::Fill::NonZero,
                        Affine::scale(scale),
                        theme.rule,
                        None,
                        &Rect::new(
                            block_left,
                            block_top + block.height / 2.0,
                            frame.right - USER_PAD_X,
                            block_top + block.height / 2.0 + frame.hairline(),
                        ),
                    );
                }
                _ => {}
            }
            let inset_y = match block.kind {
                BlockKind::CodeBlock { .. } => CODE_PAD_Y,
                _ => 0.0,
            };
            // The inset the layout wrapped to, so the drawn text cannot sit at
            // a different x than the width it was measured against.
            let inset_x = block.inset;
            // Selection bands go under the glyphs, so highlighted text stays
            // legible on the band rather than being painted over by it.
            // Inline code sits under both: it is a property of the text, so a
            // selection must read as drawn *over* the code span rather than
            // being hidden by it.
            for wash in &block.washes {
                scene.fill(
                    vello::peniko::Fill::NonZero,
                    Affine::scale(scale),
                    theme.code_wash,
                    None,
                    &RoundedRect::new(
                        text_left + inset_x + wash.x0,
                        block_top + inset_y + wash.y0,
                        text_left + inset_x + wash.x1,
                        block_top + inset_y + wash.y1,
                        crate::transcript::INLINE_CODE_RADIUS,
                    ),
                );
            }
            // Diff row bands, under everything else in the block: they say
            // which side a row is on across the card's full measure, so the
            // shape of a change is visible before a single word is read. Drawn
            // to the card's edges rather than to the text, or the band would
            // be as ragged as the code and stop being a shape at all.
            for band in &block.diff_bands {
                let color = match (band.change, band.emphasis) {
                    (crate::edits::Change::Added, false) => theme.added_wash,
                    (crate::edits::Change::Removed, false) => theme.removed_wash,
                    (crate::edits::Change::Added, true) => theme.added_mark,
                    (crate::edits::Change::Removed, true) => theme.removed_mark,
                };
                // A row band spans the card; an emphasis band hugs the glyphs
                // it marks, because *that* is the thing it is pointing at.
                let (x0, x1) = if band.emphasis {
                    (
                        text_left + inset_x + band.rect.x0,
                        text_left + inset_x + band.rect.x1,
                    )
                } else {
                    (block_left, frame.right - USER_PAD_X)
                };
                // Parley's selection rectangles stop exactly at each row's
                // floating-point edge. Rasterizing those independent edges can
                // leave a hairline of the card background between consecutive
                // diff rows, especially at fractional display scales. Row
                // washes are meant to form one continuous diff surface, so
                // bleed them by half a device pixel on each side. Keep the
                // tighter emphasis marks untouched.
                let (y0, y1) = diff_band_y(
                    band.rect,
                    block_top + inset_y,
                    frame.hairline(),
                    band.emphasis,
                );
                scene.fill(
                    vello::peniko::Fill::NonZero,
                    Affine::scale(scale),
                    color,
                    None,
                    &Rect::new(
                        x0,
                        y0,
                        x1,
                        y1,
                    ),
                );
            }
            if let Some(selection) = model.selection.as_ref()
                && let Some(range) =
                    selection.range_in(placed.index, block_index, block.source.len())
            {
                for band in crate::select::block_bands(block, range, scale) {
                    // A user message and a code block sit on a wash, so they
                    // need the stronger band: the paper-tuned one is nearly
                    // invisible against the card the user's own message is in.
                    let on_wash = is_user
                        || matches!(
                            placed.message.role,
                            Role::Tool | Role::Progress | Role::Todo
                        )
                        || matches!(block.kind, BlockKind::CodeBlock { .. });
                    let band_color = if on_wash {
                        theme.selection_on_wash
                    } else {
                        theme.selection
                    };
                    scene.fill(
                        vello::peniko::Fill::NonZero,
                        Affine::scale(scale),
                        band_color,
                        None,
                        &Rect::new(
                            text_left + inset_x + band.rect.x0,
                            block_top + inset_y + band.rect.y0,
                            text_left + inset_x + band.rect.x1,
                            block_top + inset_y + band.rect.y1,
                        ),
                    );
                }
            }
            // Reveal is expressed as a fraction of the message and applied to
            // its glyph count, because the cursor counts markdown *source*
            // characters while this draws laid-out glyphs; the two differ by
            // every `**` and backtick in the reply.
            let revealed = match streaming_index {
                Some(index) if index == placed.index => {
                    let shown = message_glyphs as f64 * model.stream.fraction();
                    (shown - drawn_glyphs as f64).max(0.0)
                }
                _ => f64::INFINITY,
            };
            if revealed <= 0.0 {
                break;
            }
            if let Some(formula) = block.math.as_ref() {
                crate::math::shared().draw(
                    scene,
                    formula,
                    (text_left + inset_x, block_top + inset_y),
                    theme.text,
                    scale,
                    revealed,
                );
            } else {
                text::TextSystem::draw_layout_revealed(
                    scene,
                    &block.layout,
                    (text_left + inset_x, block_top + inset_y),
                    scale,
                    revealed,
                );
                // Parley owns wrapping and baseline geometry for inline math.
                // Its inline boxes reserve the exact formula dimensions; draw
                // the corresponding native OpenType-MATH formula into each box.
                for line in block.layout.lines() {
                    for item in line.items() {
                        let parley::PositionedLayoutItem::InlineBox(boxed) = item else {
                            continue;
                        };
                        let Some(formula) = block.inline_math.get(boxed.id as usize) else {
                            continue;
                        };
                        crate::math::shared().draw(
                            scene,
                            formula,
                            (
                                text_left + inset_x + f64::from(boxed.x) / scale,
                                block_top + inset_y + f64::from(boxed.y) / scale,
                            ),
                            theme.text,
                            scale,
                            f64::INFINITY,
                        );
                    }
                }
            }
            drawn_glyphs += block.glyphs;
        }
        if toned {
            scene.pop_layer();
        }
    }
}

/// The one style used for composer text. Wrapping, drawing, caret placement,
/// and hit-testing must all use the same style or their geometry diverges, so
/// there is exactly one definition of it.
pub fn composer_text_style(model: &Model) -> ParagraphStyle {
    ParagraphStyle {
        font_size: layout::BODY_SIZE,
        color: model.theme.text,
        line_height: (layout::COMPOSER_LINE_HEIGHT / f64::from(layout::BODY_SIZE)) as f32,
        ..Default::default()
    }
}

/// Build the frame. `size` is the surface size in physical pixels and `scale`
/// is the window scale factor; geometry comes from [`layout::Frame`] in logical
/// units, so the design reads identically on 1x and HiDPI displays.
pub fn build_scene(
    scene: &mut Scene,
    painter: &mut crate::paint::Painter,
    model: &Model,
    size: (u32, u32),
    scale: f64,
) {
    let frame = crate::App::frame_for_model_with(size, scale, model, painter);
    let crate::paint::Painter {
        text,
        transcript: transcript_cache,
    } = painter;
    let theme = &model.theme;
    // Size the composer from where the text really wraps, via the same helper
    // the event loop uses, so pointer hit-testing can never see a different
    // frame than the renderer.
    let scale = frame.scale;
    let fill = |scene: &mut Scene, color: Color, shape: &Rect| {
        scene.fill(
            vello::peniko::Fill::NonZero,
            Affine::scale(scale),
            color,
            None,
            shape,
        );
    };
    let fill_round = |scene: &mut Scene, color: Color, shape: &RoundedRect| {
        scene.fill(
            vello::peniko::Fill::NonZero,
            Affine::scale(scale),
            color,
            None,
            shape,
        );
    };

    // Paper. Black on the opening frames of the boot reveal, then the theme's
    // own background: the window fades up from nothing rather than snapping
    // into existence fully drawn.
    let page = Rect::new(0.0, 0.0, frame.width, frame.height);
    fill(scene, model.boot.paper_color(theme.background), &page);

    // The hero donut, drawn before (and outside) the chrome reveal: during boot
    // the torus arrives first and everything else is created around it.
    let placeholder = model.transcript.is_empty();
    let hero = frame.hero().filter(|_| placeholder);
    if let (Some(hero), Some(field)) = (hero, model.donut.as_ref()) {
        draw_donut(
            scene,
            field,
            hero.donut,
            model.boot.donut_ink(theme.text, theme.background),
            scale,
            model.boot.donut(),
        );
    }

    // Everything that is not the donut fades in as one group, so the composer,
    // the wordmark, and the chrome read as one thing being created rather than
    // as several arrivals.
    match model.boot.chrome_layer() {
        crate::boot::ChromeReveal::Hidden => {
            // Help is local emergency documentation, so F1 must work even in
            // the opening frames before the ordinary chrome has appeared.
            crate::scene_help::draw_help(scene, text, model, &frame, scale);
            return;
        }
        crate::boot::ChromeReveal::Fading(alpha) => scene.push_layer(
            vello::peniko::Fill::NonZero,
            vello::peniko::Mix::Normal,
            alpha,
            Affine::scale(scale),
            &page,
        ),
        crate::boot::ChromeReveal::Solid => {}
    }
    let revealing = matches!(
        model.boot.chrome_layer(),
        crate::boot::ChromeReveal::Fading(_)
    );

    // Top chrome row: the session strip.
    if let Some(band) = frame.strip() {
        draw_strip(scene, text, model, band, &frame, scale);
    }

    // Session overview and settings, in the margin above the column's trailing
    // edge. Both stay quiet until used so they do not compete with the page.
    icons::draw(
        scene,
        icons::Icon::Sessions,
        frame.sessions(),
        if model.overview.is_open() {
            theme.text
        } else {
            theme.faint
        },
        scale,
    );

    // The settings gear, in the margin above the column's trailing edge. Faint
    // until the panel is open, when it takes full ink so the mark and the menu
    // it opened read as one thing.
    icons::draw(
        scene,
        icons::Icon::Settings,
        frame.gear(),
        if model.panel.is_open() {
            theme.text
        } else {
            theme.faint
        },
        scale,
    );

    // Composer: a real input field. Paper fill plus a hairline border, rather
    // than a grey slab: a filled block reads as disabled or as a code block,
    // while an outlined field reads as somewhere to type. The border thickens
    // when the window has focus, so focus is legible without a colour accent.
    let well = RoundedRect::new(
        frame.left,
        frame.composer_top,
        frame.right,
        frame.composer_bottom,
        layout::COMPOSER_RADIUS,
    );
    fill_round(scene, theme.field, &well);
    let (border_color, border_width) = if model.focused {
        (theme.field_border_focus, layout::COMPOSER_BORDER_FOCUS)
    } else {
        (theme.field_border, layout::COMPOSER_BORDER)
    };
    scene.stroke(
        &vello::kurbo::Stroke::new(border_width),
        Affine::scale(scale),
        border_color,
        None,
        &well,
    );

    // Transcript: ink on paper, bottom-aligned against the composer so new
    // lines rise from the well rather than dangling from the masthead.
    //
    // On an empty session the transcript region is dead space, so the hero from
    // the website lives there: the wordmark and tagline around the halftone
    // torus drawn above. It stands down the moment there is real content, so it
    // can never compete with the transcript.
    if let Some(hero) = hero {
        draw_hero_text(scene, text, model, hero, &frame, scale);
    }

    // On an empty session the hero says everything, so there is no filler
    // transcript line: a "type a message" caption next to a field that already
    // invites you to type was the same sentence twice.
    if !placeholder {
        // The transcript is the one region whose content is not bounded by the
        // layout, so it is the one region that must be clipped: without this a
        // reply too tall for its region paints straight down over the composer.
        let region = Rect::new(
            frame.left,
            frame.body_top,
            frame.right,
            frame.body_bottom.max(frame.body_top),
        );
        scene.push_clip_layer(vello::peniko::Fill::NonZero, Affine::scale(scale), &region);
        draw_transcript(scene, text, transcript_cache, model, &frame, scale);
        scene.pop_layer();
        draw_scrollbar(scene, text, transcript_cache, model, &frame, scale);
    }
    if model.model_picker.is_visible() {
        draw_model_picker(scene, text, model, &frame, scale);
    }

    // Prompt line inside the well: a real input box. The caret is drawn at
    // the measured width of the text before the cursor, so it sits between
    // glyphs and moves with Ctrl+A/E, word motion, and the arrows.
    let prompt_style = composer_text_style(model);
    let prompt_x = frame.composer_text_left();
    let preview_height = if model.attachment_previews.is_empty() {
        0.0
    } else {
        ATTACHMENT_PREVIEW_ROWS as f64 * layout::COMPOSER_LINE_HEIGHT
    };
    let prompt_y = frame.composer_top + layout::COMPOSER_TEXT_OFFSET + preview_height;
    let prompt_width = frame.composer_text_width() as f32;

    if !model.attachment_previews.is_empty() {
        let clip = RoundedRect::new(
            frame.left,
            frame.composer_top,
            frame.right,
            prompt_y,
            layout::COMPOSER_RADIUS,
        );
        scene.push_clip_layer(vello::peniko::Fill::NonZero, Affine::scale(scale), &clip);
        for (image, rect) in model
            .attachment_previews
            .iter()
            .zip(attachment_thumbnail_rects(&frame, model))
        {
            scene.draw_image(
                image,
                Affine::scale(scale)
                    * Affine::translate((rect.x0, rect.y0))
                    * Affine::scale_non_uniform(
                        rect.width() / image.width.max(1) as f64,
                        rect.height() / image.height.max(1) as f64,
                    ),
            );
            draw_close(
                scene,
                attachment_thumbnail_close_rect(rect),
                scale,
                theme.background.with_alpha(0.82),
                theme.text,
            );
        }
        scene.pop_layer();
    }

    {
        // One Parley layout drives wrapping, the selection bands, the glyphs,
        // and the caret, so the three can never disagree: the highlight lines
        // up with the text because it *is* the text's own geometry.
        let source = model.editor.text();
        let input = crate::input::InputLayout::new(
            text,
            source,
            frame.composer_text_width(),
            prompt_style,
            scale,
        );
        // Scroll the well to the caret's line when the text is taller than the
        // well, so typing never runs out of sight.
        let visible_lines =
            frame
                .composer_lines()
                .saturating_sub(if model.attachment_previews.is_empty() {
                    0
                } else {
                    ATTACHMENT_PREVIEW_ROWS
                });
        let origin_y = prompt_y - input.scroll_offset(model.editor.cursor(), visible_lines);
        let clip_top = frame.composer_top;
        let clip_bottom = frame.composer_bottom;

        // Selection bands, under the glyphs so text on them stays legible.
        if let Some((sel_start, sel_end)) = model.editor.selection() {
            for band in input.selection_rects(sel_start, sel_end) {
                let top = origin_y + band.y0;
                let bottom = origin_y + band.y1;
                if bottom <= clip_top || top >= clip_bottom {
                    continue;
                }
                fill(
                    scene,
                    theme.selection,
                    &Rect::new(
                        (prompt_x + band.x0).min(frame.right),
                        top.max(clip_top),
                        (prompt_x + band.x1).min(frame.right),
                        bottom.min(clip_bottom),
                    ),
                );
            }
        }

        // An empty field carries a rotating invitation rather than a label:
        // "message jcode" is a caption you stop seeing, while a prompt you
        // could actually type teaches what the thing is for.
        //
        // The field never carries the turn's status: liveness belongs to the
        // transcript's live tool card and the window's own busy cues, and a
        // phase/elapsed line here fought the caret and the next message the
        // user was already typing.
        if model.editor.is_empty() {
            let hint = crate::input::InputLayout::new(
                text,
                crate::hints::hint(model.hint),
                f64::from(prompt_width),
                ParagraphStyle {
                    color: theme.faint,
                    ..prompt_style
                },
                scale,
            );
            let band = Rect::new(
                frame.left,
                prompt_y,
                frame.right,
                (prompt_y + frame.composer_lines() as f64 * layout::COMPOSER_LINE_HEIGHT)
                    .min(clip_bottom),
            );
            scene.push_clip_layer(vello::peniko::Fill::NonZero, Affine::scale(scale), &band);
            crate::text::TextSystem::draw_layout(scene, hint.layout(), (prompt_x, prompt_y), scale);
            scene.pop_layer();
        } else {
            // Draw the whole layout in one pass: Parley already wrapped it to
            // the well, so per-row drawing would only reintroduce drift.
            // Clipped to the text band, not the whole well: the layout is
            // scrolled under the field once it outgrows it, and clipping to
            // the well would let the row above bleed a sliced half-glyph into
            // the top padding. The band is a whole number of rows, so the
            // window always shows whole lines.
            let band = Rect::new(
                frame.left,
                prompt_y,
                frame.right,
                (prompt_y + frame.composer_lines() as f64 * layout::COMPOSER_LINE_HEIGHT)
                    .min(clip_bottom),
            );
            scene.push_clip_layer(vello::peniko::Fill::NonZero, Affine::scale(scale), &band);
            crate::text::TextSystem::draw_layout(
                scene,
                input.layout(),
                (prompt_x, origin_y),
                scale,
            );
            scene.pop_layer();
        }

        // An unfocused window must not show a blinking caret: it would claim
        // keystrokes land here when they do not.
        if model.focused && model.caret.visible() {
            let bar = input.caret_rect(model.editor.cursor(), layout::CARET_WIDTH);
            let top = (origin_y + bar.y0).max(clip_top);
            let bottom = (origin_y + bar.y1).min(clip_bottom);
            let caret_x = (prompt_x + bar.x0).min(frame.right - layout::CARET_WIDTH);
            if bottom > top {
                fill(
                    scene,
                    theme.text,
                    &Rect::new(caret_x, top, caret_x + layout::CARET_WIDTH, bottom),
                );
            }
        }
    }

    // A transient notice, or a scrollback indicator, as a caption under the
    // well. Never covers content.
    // The model decides *what* to say (see `Model::footnote`); this only
    // decides how wide it may be. Status and build alerts live here instead of
    // a masthead, so the top of the page stays clear while a failure to attach
    // is still visible.
    let footnote = model.footnote().map(|line| {
        let chars = (frame.column() / (f64::from(layout::CAPTION_SIZE) * 0.72)) as usize;
        elide(&line, chars.max(12))
    });
    if let Some(footnote) = footnote {
        text.draw_paragraph_scaled(
            scene,
            &footnote,
            (frame.left, frame.footnote_top),
            frame.column() as f32,
            ParagraphStyle {
                font_size: layout::CAPTION_SIZE,
                color: theme.faint,
                letter_spacing_em: 0.1,
                ..Default::default()
            },
            scale,
        );
    }

    // The settings panel sits over the page, under the overview: it is a
    // menu hanging off the gear, so it is drawn after the content it covers
    // and before the mode that would replace the whole window.
    if model.panel.is_open() {
        draw_settings_panel(scene, text, model, &frame, scale);
    }

    // The session overview sits over everything as a bounded panel. Drawing it
    // last preserves the current transcript behind its light scrim.
    if model.overview.is_visible() {
        crate::scene_overview::draw_overview(
            scene,
            text,
            model,
            &frame,
            scale,
            std::time::Instant::now(),
        );
    }

    // The resume overlay is drawn last of all: it is a mode over the page, and
    // unlike the overview it deliberately leaves the conversation legible
    // underneath, which only works if nothing paints over it afterwards.
    crate::scene_resume::draw_resume(scene, text, model, &frame, scale);

    if let Some((index, opened, automatic)) = model.attachment_preview
        && let Some(image) = model.attachment_previews.get(index)
    {
        let elapsed = std::time::Instant::now().saturating_duration_since(opened);
        let transition = if automatic {
            ((elapsed.as_secs_f64() - 0.65) / 0.40).clamp(0.0, 1.0)
        } else {
            0.0
        };
        if !automatic || transition < 1.0 {
            let max_w = frame.width * 0.72;
            let max_h = frame.height * 0.68;
            let fit = (max_w / image.width.max(1) as f64).min(max_h / image.height.max(1) as f64);
            let large_w = image.width as f64 * fit;
            let large_h = image.height as f64 * fit;
            let large = Rect::new(
                (frame.width - large_w) * 0.5,
                (frame.height - large_h) * 0.42,
                (frame.width + large_w) * 0.5,
                (frame.height - large_h) * 0.42 + large_h,
            );
            let target = attachment_thumbnail_rects(&frame, model)
                .get(index)
                .copied()
                .unwrap_or(large);
            let eased = transition * transition * (3.0 - 2.0 * transition);
            let lerp = |a: f64, b: f64| a + (b - a) * eased;
            let shown = Rect::new(
                lerp(large.x0, target.x0),
                lerp(large.y0, target.y0),
                lerp(large.x1, target.x1),
                lerp(large.y1, target.y1),
            );
            scene.fill(
                vello::peniko::Fill::NonZero,
                Affine::scale(scale),
                theme.background.with_alpha((0.76 * (1.0 - eased)) as f32),
                None,
                &Rect::new(0.0, 0.0, frame.width, frame.height),
            );
            scene.draw_image(
                image,
                Affine::scale(scale)
                    * Affine::translate((shown.x0, shown.y0))
                    * Affine::scale_non_uniform(
                        shown.width() / image.width.max(1) as f64,
                        shown.height() / image.height.max(1) as f64,
                    ),
            );
            if eased < 0.55 {
                draw_close(
                    scene,
                    attachment_overlay_close_rect(&frame),
                    scale,
                    theme.field,
                    theme.text,
                );
            }
        }
    }

    if revealing {
        scene.pop_layer();
    }

    // Draw outside the boot reveal layer and after every other overlay. Help is
    // a modal reference, not part of the page fading in underneath it.
    crate::scene_help::draw_help(scene, text, model, &frame, scale);
}

/// Middle-elide `text` to at most `max_chars` characters, keeping the head and
/// tail (the informative ends of paths, ids, and error strings).
pub fn elide(text: &str, max_chars: usize) -> String {
    let text = text.trim();
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max_chars {
        return text.to_string();
    }
    if max_chars <= 3 {
        return "...".to_string();
    }
    let keep = max_chars - 3;
    let head = keep.div_ceil(2);
    let tail = keep - head;
    let mut out: String = chars[..head].iter().collect();
    out.push_str("...");
    out.extend(&chars[chars.len() - tail..]);
    out
}

#[cfg(test)]
mod tests {
    use super::{diff_band_y, elide};

    #[test]
    fn elide_keeps_short_text() {
        assert_eq!(elide("attached", 20), "attached");
    }

    #[test]
    fn elide_respects_budget_and_keeps_ends() {
        let out = elide("disconnected: no such file or directory (os error 2)", 24);
        assert_eq!(out.chars().count(), 24);
        assert!(out.starts_with("disconn"));
        assert!(out.ends_with("2)"));
    }

    #[test]
    fn elide_handles_tiny_budget() {
        assert_eq!(elide("abcdef", 2), "...");
    }

    #[test]
    fn adjacent_diff_row_washes_overlap_by_a_device_pixel() {
        use vello::kurbo::Rect;

        // Integer and fractional HiDPI scales. `hairline` is one physical pixel
        // in logical coordinates, exactly as Frame supplies it to painting.
        for scale in [1.0, 1.25, 1.5, 1.75, 2.0, 2.5] {
            let hairline = 1.0 / scale;
            let origin = 13.37;
            let (_, first_bottom) = diff_band_y(
                Rect::new(0.0, 0.0, 100.0, 19.2),
                origin,
                hairline,
                false,
            );
            let (second_top, _) = diff_band_y(
                Rect::new(0.0, 19.2, 100.0, 38.4),
                origin,
                hairline,
                false,
            );
            let overlap_px = (first_bottom - second_top) * scale;
            assert!(
                (overlap_px - 1.0).abs() < 1e-9,
                "scale {scale}: row washes overlap by {overlap_px} device pixels"
            );
        }
    }

    #[test]
    fn diff_emphasis_marks_keep_exact_text_geometry() {
        let rect = vello::kurbo::Rect::new(2.0, 3.0, 7.0, 11.0);
        assert_eq!(diff_band_y(rect, 20.0, 0.5, true), (23.0, 31.0));
    }
}
