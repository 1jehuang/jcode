//! Frame construction: the pure `Model` -> `Scene` function.
//!
//! Kept separate from the event loop so a frame is a pure function of the
//! model, which is what makes the state-space captures and pixel tests
//! possible.

use crate::text::ParagraphStyle;
use crate::{Model, donut, layout, text};
use vello::Scene;
use vello::kurbo::{Affine, BezPath, Circle, Rect, RoundedRect, Shape};
use vello::peniko::Color;

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

fn draw_donut(scene: &mut Scene, field: &donut::Donut, box_: Rect, ink: Color, scale: f64) {
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
    scene.fill(
        vello::peniko::Fill::NonZero,
        Affine::scale(scale),
        ink,
        None,
        &dots,
    );
}

/// Draw the hero block shown on an empty session: the wordmark, the halftone
/// donut, and one line of invitation, stacked and centred exactly like the
/// website's landing section.
fn draw_hero(
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
    if let Some(field) = model.donut.as_ref() {
        draw_donut(scene, field, hero.donut, model.theme.text, scale);
    }
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

/// Draw the session strip: a row of bars at the top of the window, one per
/// live session, grouped by working directory.
///
/// Deliberately the same visual language as the author's waybar
/// `niri-workspaces` module, because it is the language he already reads
/// without thinking: a dim group label, then thin ticks for the sessions in
/// it, with the focused one a wide solid block.
fn draw_strip(
    scene: &mut Scene,
    text: &mut text::TextSystem,
    model: &Model,
    band: (f64, f64),
    frame: &layout::Frame,
    scale: f64,
) {
    let (top, bottom) = band;
    let label_style = ParagraphStyle {
        font_size: layout::STRIP_LABEL_SIZE,
        color: model.theme.faint,
        letter_spacing_em: 0.08,
        line_height: 1.0,
        ..Default::default()
    };
    // Measure labels through the same text system that draws them, so the bars
    // sit where the label really ends rather than at an estimate. Measured up
    // front because layout needs them all and the text system is exclusive.
    let widths: Vec<(String, f64)> = model
        .strip
        .groups()
        .iter()
        .map(|group| {
            (
                group.label.clone(),
                text.measure_width(&group.label, label_style, scale),
            )
        })
        .collect();
    let items = crate::strip::layout_items(&model.strip, frame.left, frame.right, |label| {
        widths
            .iter()
            .find(|(name, _)| name == label)
            .map(|(_, width)| *width)
            .unwrap_or(0.0)
    });

    // Bars are centred in the band; the label sits on the same row.
    let bar_top = top + (bottom - top - layout::STRIP_BAR_HEIGHT) / 2.0;
    let label_top = top + (bottom - top - f64::from(layout::STRIP_LABEL_SIZE)) / 2.0;

    for item in items {
        match item {
            crate::strip::Item::Label { group, x } => {
                let Some(group) = model.strip.groups().get(group) else {
                    continue;
                };
                text.draw_paragraph_scaled(
                    scene,
                    &group.label,
                    (x, label_top),
                    frame.column() as f32,
                    label_style,
                    scale,
                );
            }
            crate::strip::Item::Bar {
                x,
                width,
                focused,
                group,
                index,
            } => {
                // Unfocused bars are dim so the focused one reads instantly;
                // a busy session is drawn at full ink even when unfocused, so
                // work happening off-screen is visible rather than silent.
                let busy = model
                    .strip
                    .groups()
                    .get(group)
                    .and_then(|g| g.entries.get(index))
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
                    &RoundedRect::new(
                        x,
                        bar_top,
                        x + width,
                        bar_top + layout::STRIP_BAR_HEIGHT,
                        1.0,
                    ),
                );
            }
        }
    }
}

/// The tagline under the donut, matching the website's hero copy.
const HERO_TAGLINE: &str = "an open source coding agent, written in rust";

/// Draw the working directory and the RAM readout on the trailing end of the
/// top chrome row.
///
/// Right-aligned against the strip's bars so the row reads as "these sessions,
/// this place": the answer to "which checkout am I talking to" was previously
/// only inferable from the strip's leaf labels, and not available at all in a
/// single-session window. Head-elided rather than middle-elided because the
/// tail of a path is the part that identifies it. The RAM caption sits after
/// the path (`ui 105 MB · srv 428 MB`): it is metadata about the same corner
/// of the app, and a second row for it would spend a whole band on two
/// numbers.
fn draw_place(
    scene: &mut Scene,
    text: &mut text::TextSystem,
    model: &Model,
    band: (f64, f64),
    frame: &layout::Frame,
    scale: f64,
) {
    let path = model
        .working_dir
        .as_deref()
        .map(|dir| crate::place::display_path(dir, crate::place::home().as_deref()))
        .filter(|path| !path.is_empty());
    let mem = model.mem.as_ref().map(crate::mem::Readout::caption);
    let (top, bottom) = band;
    let style = ParagraphStyle {
        font_size: layout::STRIP_LABEL_SIZE,
        color: model.theme.faint,
        letter_spacing_em: 0.08,
        line_height: 1.0,
        align: text::Align::End,
        ..Default::default()
    };
    // Never more than half the row: the strip is the interactive half and must
    // not be pushed off the page by a deep path. The RAM caption is small and
    // fixed-format, so it is kept whole and the path absorbs the elision.
    let budget = (frame.column() / (f64::from(layout::STRIP_LABEL_SIZE) * 0.62) / 2.0) as usize;
    let label = match (path, mem) {
        (Some(path), Some(mem)) => {
            let path_budget = budget.max(8).saturating_sub(mem.chars().count() + 3);
            if path_budget >= 8 {
                format!("{}  ·  {mem}", elide_head(&path, path_budget))
            } else {
                mem
            }
        }
        (Some(path), None) => elide_head(&path, budget.max(8)),
        (None, Some(mem)) => mem,
        (None, None) => return,
    };
    let label_top = top + (bottom - top - f64::from(layout::STRIP_LABEL_SIZE)) / 2.0;
    text.draw_paragraph_scaled(
        scene,
        &label,
        (frame.left, label_top),
        frame.column() as f32,
        style,
        scale,
    );
}

/// Elide `text` from the left, keeping the tail: `.../crates/jcode-desktop2`.
pub fn elide_head(text: &str, max_chars: usize) -> String {
    let text = text.trim();
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max_chars {
        return text.to_string();
    }
    if max_chars <= 3 {
        return "...".to_string();
    }
    let keep = max_chars - 3;
    let mut out = String::from("...");
    out.extend(&chars[chars.len() - keep..]);
    out
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
    // `Transcript::streaming_len` counts, or the two would disagree).
    let streaming_index = laid
        .iter()
        .rposition(|message| !matches!(message.role, Role::Tool | Role::Notice))
        .filter(|_| model.stream.is_revealing())
        .filter(|index| laid[*index].role != Role::User);

    for placed in &view.visible {
        let message_top = frame.body_top + placed.top;
        let is_user = placed.message.role == Role::User;
        // The user's card: the same fill and radius as the composer, so the
        // message and the field it came from are visibly one object.
        if is_user {
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
        }
        let text_left = frame.left + USER_PAD_X;
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

        // A failure notice: a rule down its left edge, no wash. A washed card
        // is the user's own message in this theme, and dressing an error as
        // something the user typed is worse than not marking it at all. The
        // rule is the print convention for an interjection, and it takes the
        // error ink so the mark is as loud as the text it labels.
        if placed.message.role == Role::Notice {
            scene.fill(
                vello::peniko::Fill::NonZero,
                Affine::scale(scale),
                theme.error,
                None,
                &Rect::new(
                    frame.left + USER_PAD_X,
                    message_top,
                    frame.left + USER_PAD_X + frame.hairline() * 2.0,
                    message_top + placed.message.height,
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

        for (block_index, block) in placed.message.blocks.iter().enumerate() {
            let block_top = text_top + block.top;
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
                            text_left,
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
                            text_left,
                            block_top,
                            text_left + frame.hairline() * 2.0,
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
                            text_left,
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
            if let Some(selection) = model.selection.as_ref()
                && let Some(range) =
                    selection.range_in(placed.index, block_index, block.source.len())
            {
                for band in crate::select::block_bands(block, range, scale) {
                    // A user message and a code block sit on a wash, so they
                    // need the stronger band: the paper-tuned one is nearly
                    // invisible against the card the user's own message is in.
                    let on_wash = is_user
                        || placed.message.role == Role::Tool
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
            text::TextSystem::draw_layout_revealed(
                scene,
                &block.layout,
                (text_left + inset_x, block_top + inset_y),
                scale,
                revealed,
            );
            drawn_glyphs += block.glyphs;
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

    // Paper.
    fill(
        scene,
        theme.background,
        &Rect::new(0.0, 0.0, frame.width, frame.height),
    );

    // Top chrome row: the session strip on the left, and where this window is
    // on the right.
    if let Some(band) = frame.strip() {
        draw_strip(scene, text, model, band, &frame, scale);
        draw_place(scene, text, model, band, &frame, scale);
    }

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
    let placeholder = model.transcript.is_empty();

    // On an empty session the transcript region is dead space, so the hero
    // donut from the website lives there: the same halftone torus, and
    // draggable in the same way. It stands down the moment there is real
    // content, so it can never compete with the transcript.
    if let Some(hero) = frame.hero().filter(|_| placeholder) {
        draw_hero(scene, text, model, hero, &frame, scale);
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

    // Prompt line inside the well: a real input box. The caret is drawn at
    // the measured width of the text before the cursor, so it sits between
    // glyphs and moves with Ctrl+A/E, word motion, and the arrows.
    let prompt_style = composer_text_style(model);
    let prompt_x = frame.composer_text_left();
    let prompt_y = frame.composer_top + layout::COMPOSER_TEXT_OFFSET;
    let prompt_width = frame.composer_text_width() as f32;

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
        let origin_y =
            prompt_y - input.scroll_offset(model.editor.cursor(), frame.composer_lines());
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
            text.draw_paragraph_scaled(
                scene,
                crate::hints::hint(model.hint),
                (prompt_x, prompt_y),
                prompt_width,
                ParagraphStyle {
                    color: theme.faint,
                    ..prompt_style
                },
                scale,
            );
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
    // Elided to a third of the column: a route-prefixed model id can be long,
    // and it must never crowd out the footnote, which is the actionable half.
    let model_caption = model.model.as_ref().and_then(|id| id.caption()).map(|id| {
        let chars = (frame.column() / (f64::from(layout::CAPTION_SIZE) * 0.72) / 3.0) as usize;
        elide(&id, chars.max(10))
    });
    let footnote = model.footnote().map(|line| {
        let chars = (frame.column() / (f64::from(layout::CAPTION_SIZE) * 0.72)) as usize;
        // Halve the budget when the model caption shares the row, so the two
        // captions cannot overlap in the middle.
        let chars = if model_caption.is_some() {
            chars / 2
        } else {
            chars
        };
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

    // Which model is answering, as a caption on the trailing end of the
    // footnote row. Right-aligned so it reads as metadata about the session
    // rather than as another message to the user, and drawn after the footnote
    // so a long notice is the thing that gets elided, not this.
    if let Some(caption) = model_caption {
        text.draw_paragraph_scaled(
            scene,
            &caption,
            (frame.left, frame.footnote_top),
            frame.column() as f32,
            ParagraphStyle {
                font_size: layout::CAPTION_SIZE,
                color: theme.faint,
                letter_spacing_em: 0.1,
                align: text::Align::End,
                ..Default::default()
            },
            scale,
        );
    }

    // The session overview sits over everything: it is a mode, not a panel,
    // and drawing it last is what lets it wash the page it replaces.
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
    use super::elide;

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
}
