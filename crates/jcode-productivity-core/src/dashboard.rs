//! Build a shareable dashboard as an SVG and rasterize it to PNG.
//!
//! The SVG is assembled by hand (no templating dep) into a dark, card-based
//! "wrapped"-style poster, then rendered to a PNG via resvg/usvg/tiny-skia using
//! the system font database.

use crate::markdown::human;
use crate::model::ProductivityReport;
use anyhow::{Context, Result};
use std::sync::{Arc, LazyLock};

const W: f32 = 1200.0;
const H: f32 = 1440.0;

// Palette (dark, high-contrast, screenshot-friendly).
const BG: &str = "#0b0f17";
const CARD: &str = "#151b27";
const CARD2: &str = "#1b2230";
const ACCENT: &str = "#7c9cff";
const ACCENT2: &str = "#5ce1a6";
const ACCENT3: &str = "#ffb86b";
const TEXT: &str = "#e8edf6";
const MUTED: &str = "#8a94a7";
const TRACK: &str = "#222a3a";

static FONT_DB: LazyLock<Arc<usvg::fontdb::Database>> = LazyLock::new(|| {
    let mut db = usvg::fontdb::Database::new();
    db.load_system_fonts();
    Arc::new(db)
});

/// Escape text for inclusion in SVG element bodies/attributes.
fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Truncate to a max display length with an ellipsis.
fn clip(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        s.to_string()
    } else {
        let mut out: String = chars[..max.saturating_sub(1)].iter().collect();
        out.push('…');
        out
    }
}

/// Drop emoji / pictographic codepoints the system sans font can't render, so
/// the PNG never shows tofu boxes. Keeps ASCII + common latin/punctuation.
fn ascii_only(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        let keep = c.is_ascii() || matches!(c, '…' | '·' | '–' | '—' | '’' | '“' | '”');
        if keep {
            out.push(c);
        }
    }
    // Collapse runs of whitespace left behind by removed emoji.
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

struct Svg {
    body: String,
}

impl Svg {
    fn new() -> Self {
        Self {
            body: String::with_capacity(16 * 1024),
        }
    }

    fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, fill: &str) {
        self.body.push_str(&format!(
            "<rect x='{x:.1}' y='{y:.1}' width='{w:.1}' height='{h:.1}' rx='{r:.1}' ry='{r:.1}' fill='{fill}'/>"
        ));
    }

    fn rect_op(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, fill: &str, opacity: f32) {
        self.body.push_str(&format!(
            "<rect x='{x:.1}' y='{y:.1}' width='{w:.1}' height='{h:.1}' rx='{r:.1}' ry='{r:.1}' fill='{fill}' fill-opacity='{opacity:.2}'/>"
        ));
    }

    #[allow(clippy::too_many_arguments)]
    fn text(&mut self, x: f32, y: f32, s: &str, size: f32, fill: &str, weight: u32, anchor: &str) {
        self.body.push_str(&format!(
            "<text x='{x:.1}' y='{y:.1}' font-family='Inter, \"Liberation Sans\", \"DejaVu Sans\", sans-serif' font-size='{size:.1}' font-weight='{weight}' fill='{fill}' text-anchor='{anchor}'>{}</text>",
            esc(s)
        ));
    }

    /// Draw a small lightning bolt glyph whose bounding box is roughly
    /// `size` tall, with its top-left at (x, y).
    fn bolt(&mut self, x: f32, y: f32, size: f32, fill: &str) {
        let s = size;
        // Simple zig-zag bolt polygon.
        let pts = [
            (0.55, 0.0),
            (0.10, 0.58),
            (0.45, 0.58),
            (0.30, 1.0),
            (0.90, 0.40),
            (0.52, 0.40),
            (0.80, 0.0),
        ];
        let path: String = pts
            .iter()
            .map(|(px, py)| format!("{:.1},{:.1}", x + px * s, y + py * s))
            .collect::<Vec<_>>()
            .join(" ");
        self.body
            .push_str(&format!("<polygon points='{path}' fill='{fill}'/>"));
    }
}

/// Generate the dashboard SVG string for a report.
pub fn render_svg(r: &ProductivityReport) -> String {
    let mut svg = Svg::new();

    // Background with a subtle top gradient bar.
    svg.rect(0.0, 0.0, W, H, 0.0, BG);
    svg.body.push_str(&format!(
        "<defs><linearGradient id='hd' x1='0' y1='0' x2='1' y2='0'>\
         <stop offset='0' stop-color='{ACCENT}'/>\
         <stop offset='0.5' stop-color='{ACCENT2}'/>\
         <stop offset='1' stop-color='{ACCENT3}'/></linearGradient></defs>"
    ));
    svg.rect(0.0, 0.0, W, 10.0, 0.0, "url(#hd)");

    let pad = 56.0;

    // ---- Header ----
    svg.text(pad, 92.0, "jcode", 30.0, ACCENT, 800, "start");
    svg.text(pad, 92.0 + 0.0, "", 1.0, TEXT, 400, "start");
    svg.text(
        W - pad,
        70.0,
        "PRODUCTIVITY REPORT",
        18.0,
        MUTED,
        700,
        "end",
    );
    svg.text(W - pad, 96.0, &r.generated_at, 16.0, MUTED, 400, "end");

    svg.text(pad, 158.0, &r.archetype, 52.0, TEXT, 800, "start");
    svg.text(
        pad,
        196.0,
        &clip(&r.archetype_blurb, 78),
        20.0,
        MUTED,
        400,
        "start",
    );

    // Power score pill (right aligned).
    let pill_w = 320.0;
    let pill_x = W - pad - pill_w;
    svg.rect(pill_x, 128.0, pill_w, 78.0, 18.0, CARD2);
    svg.text(
        pill_x + 24.0,
        160.0,
        "POWER SCORE",
        15.0,
        MUTED,
        700,
        "start",
    );
    let score = human(r.power_score);
    svg.text(
        pill_x + pill_w - 24.0,
        186.0,
        &score,
        38.0,
        ACCENT2,
        800,
        "end",
    );
    // Lightning bolt to the left of the score number.
    let score_w = score.chars().count() as f32 * 22.0;
    svg.bolt(
        pill_x + pill_w - 24.0 - score_w - 34.0,
        154.0,
        34.0,
        ACCENT3,
    );

    // ---- Stat cards grid ----
    let grid_top = 240.0;
    let cols = 4usize;
    let gap = 20.0;
    let card_w = (W - 2.0 * pad - gap * (cols as f32 - 1.0)) / cols as f32;
    let card_h = 118.0;

    let cards: Vec<(String, String, &str)> = vec![
        (human(r.total_sessions), "Sessions".into(), ACCENT),
        (human(r.user_prompts), "Prompts".into(), ACCENT),
        (human(r.total_tool_calls), "Tool calls".into(), ACCENT2),
        (human(r.code_edits), "Code edits".into(), ACCENT2),
        (human(r.output_tokens), "Tokens out".into(), ACCENT3),
        (human(r.input_tokens), "Tokens in".into(), ACCENT3),
        (human(r.cache_read_tokens), "Cache reads".into(), ACCENT3),
        (human(r.distinct_projects), "Projects".into(), ACCENT),
        (r.active_days.to_string(), "Active days".into(), ACCENT2),
        (
            format!("{}", r.longest_streak),
            "Best streak (days)".into(),
            ACCENT3,
        ),
        (human(r.commands_run), "Commands".into(), ACCENT),
        (human(r.searches), "Searches".into(), ACCENT2),
    ];

    for (i, (value, label, color)) in cards.iter().enumerate() {
        let col = i % cols;
        let row = i / cols;
        let x = pad + col as f32 * (card_w + gap);
        let y = grid_top + row as f32 * (card_h + gap);
        svg.rect(x, y, card_w, card_h, 16.0, CARD);
        svg.text(x + 22.0, y + 56.0, value, 38.0, color, 800, "start");
        svg.text(x + 22.0, y + 90.0, label, 17.0, MUTED, 500, "start");
    }

    let after_grid = grid_top + 3.0 * (card_h + gap) + 16.0;

    // ---- Two-column section: hour rhythm + weekday ----
    let sec_h = 250.0;
    let half_w = (W - 2.0 * pad - gap) / 2.0;

    // Hour-of-day chart.
    let hx = pad;
    let hy = after_grid;
    svg.rect(hx, hy, half_w, sec_h, 16.0, CARD);
    svg.text(
        hx + 22.0,
        hy + 38.0,
        "Daily rhythm",
        22.0,
        TEXT,
        700,
        "start",
    );
    svg.text(
        hx + half_w - 22.0,
        hy + 38.0,
        &format!("peak {} ", crate::markdown::hour_label_pub(r.peak_hour)),
        16.0,
        MUTED,
        500,
        "end",
    );
    draw_hour_bars(
        &mut svg,
        hx + 22.0,
        hy + 62.0,
        half_w - 44.0,
        150.0,
        &r.hour_hist,
    );

    // Weekday chart.
    let wx = pad + half_w + gap;
    let wy = after_grid;
    svg.rect(wx, wy, half_w, sec_h, 16.0, CARD);
    svg.text(wx + 22.0, wy + 38.0, "By weekday", 22.0, TEXT, 700, "start");
    draw_weekday_bars(
        &mut svg,
        wx + 22.0,
        wy + 62.0,
        half_w - 44.0,
        150.0,
        &r.weekday_hist,
    );

    let after_charts = after_grid + sec_h + gap;

    // ---- Two-column: top tools + top projects ----
    let list_h = 330.0;
    let tx = pad;
    let ty = after_charts;
    svg.rect(tx, ty, half_w, list_h, 16.0, CARD);
    svg.text(
        tx + 22.0,
        ty + 38.0,
        "Most-used tools",
        22.0,
        TEXT,
        700,
        "start",
    );
    draw_tool_list(&mut svg, tx + 22.0, ty + 64.0, half_w - 44.0, &r.top_tools);

    let px = pad + half_w + gap;
    let py = after_charts;
    svg.rect(px, py, half_w, list_h, 16.0, CARD);
    svg.text(
        px + 22.0,
        py + 38.0,
        "Top projects",
        22.0,
        TEXT,
        700,
        "start",
    );
    draw_project_list(
        &mut svg,
        px + 22.0,
        py + 64.0,
        half_w - 44.0,
        &r.top_projects,
    );

    let after_lists = after_charts + list_h + gap;

    // ---- Badges strip ----
    if !r.badges.is_empty() {
        let by = after_lists;
        let bh = 70.0;
        svg.rect(pad, by, W - 2.0 * pad, bh, 16.0, CARD2);
        let mut bx = pad + 22.0;
        for badge in r.badges.iter() {
            let label = clip(&ascii_only(badge), 24);
            if label.is_empty() {
                continue;
            }
            let chip_w = 30.0 + label.chars().count() as f32 * 10.5;
            if bx + chip_w > W - pad - 20.0 {
                break;
            }
            svg.rect_op(bx, by + 16.0, chip_w, 38.0, 19.0, ACCENT, 0.14);
            svg.text(bx + 16.0, by + 41.0, &label, 17.0, TEXT, 600, "start");
            bx += chip_w + 14.0;
        }
    }

    // ---- Footer ----
    svg.text(
        pad,
        H - 30.0,
        &format!(
            "{} active days · {} day span · chronotype: {}",
            r.active_days, r.span_days, r.chronotype
        ),
        16.0,
        MUTED,
        400,
        "start",
    );
    svg.text(
        W - pad,
        H - 30.0,
        "generated with jcode  ·  /productivity",
        16.0,
        ACCENT,
        600,
        "end",
    );

    format!(
        "<svg xmlns='http://www.w3.org/2000/svg' width='{W}' height='{H}' viewBox='0 0 {W} {H}'>{}</svg>",
        svg.body
    )
}

fn draw_hour_bars(svg: &mut Svg, x: f32, y: f32, w: f32, h: f32, hist: &[u32; 24]) {
    let max = hist.iter().copied().max().unwrap_or(1).max(1) as f32;
    let n = 24usize;
    let gap = 4.0;
    let bar_w = (w - gap * (n as f32 - 1.0)) / n as f32;
    for (i, &v) in hist.iter().enumerate() {
        let bh = (v as f32 / max) * h;
        let bx = x + i as f32 * (bar_w + gap);
        // track
        svg.rect(bx, y, bar_w, h, 3.0, TRACK);
        if bh > 0.5 {
            svg.rect(bx, y + (h - bh), bar_w, bh, 3.0, ACCENT);
        }
    }
    // hour ticks
    for &hh in &[0usize, 6, 12, 18, 23] {
        let bx = x + hh as f32 * (bar_w + gap) + bar_w / 2.0;
        svg.text(
            bx,
            y + h + 22.0,
            &format!("{}h", hh),
            13.0,
            MUTED,
            400,
            "middle",
        );
    }
}

fn draw_weekday_bars(svg: &mut Svg, x: f32, y: f32, w: f32, h: f32, hist: &[u32; 7]) {
    let labels = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    let max = hist.iter().copied().max().unwrap_or(1).max(1) as f32;
    let n = 7usize;
    let gap = 14.0;
    let bar_w = (w - gap * (n as f32 - 1.0)) / n as f32;
    for i in 0..n {
        let v = hist[i] as f32;
        let bh = (v / max) * h;
        let bx = x + i as f32 * (bar_w + gap);
        svg.rect(bx, y, bar_w, h, 4.0, TRACK);
        if bh > 0.5 {
            let color = if i >= 5 { ACCENT3 } else { ACCENT2 };
            svg.rect(bx, y + (h - bh), bar_w, bh, 4.0, color);
        }
        svg.text(
            bx + bar_w / 2.0,
            y + h + 22.0,
            labels[i],
            13.0,
            MUTED,
            400,
            "middle",
        );
    }
}

fn draw_tool_list(svg: &mut Svg, x: f32, y: f32, w: f32, tools: &[crate::model::Tally]) {
    let max = tools.first().map(|t| t.count).unwrap_or(1).max(1) as f32;
    let row_h = 34.0;
    let label_w = 110.0;
    let val_w = 70.0;
    let bar_x = x + label_w;
    let bar_w = w - label_w - val_w;
    for (i, t) in tools.iter().take(8).enumerate() {
        let ry = y + i as f32 * row_h;
        svg.text(x, ry + 6.0, &clip(&t.name, 12), 16.0, TEXT, 500, "start");
        svg.rect(bar_x, ry - 10.0, bar_w, 16.0, 8.0, TRACK);
        let fill = (t.count as f32 / max) * bar_w;
        if fill > 1.0 {
            svg.rect(bar_x, ry - 10.0, fill, 16.0, 8.0, ACCENT);
        }
        svg.text(x + w, ry + 6.0, &human(t.count), 15.0, MUTED, 600, "end");
    }
}

fn draw_project_list(svg: &mut Svg, x: f32, y: f32, w: f32, projects: &[crate::model::Tally]) {
    let row_h = 34.0;
    for (i, p) in projects.iter().take(8).enumerate() {
        let ry = y + i as f32 * row_h;
        svg.text(
            x,
            ry + 6.0,
            &format!("{}. {}", i + 1, clip(&p.name, 22)),
            16.0,
            TEXT,
            500,
            "start",
        );
        svg.text(
            x + w,
            ry + 6.0,
            &format!("{}", p.count),
            15.0,
            MUTED,
            600,
            "end",
        );
    }
}

/// Rasterize the dashboard SVG to PNG bytes.
pub fn render_png(r: &ProductivityReport) -> Result<Vec<u8>> {
    let svg = render_svg(r);
    let opt = usvg::Options {
        font_family: "Inter".to_string(),
        fontdb: FONT_DB.clone(),
        ..Default::default()
    };
    let tree = usvg::Tree::from_str(&svg, &opt).context("parse dashboard svg")?;
    let size = tree.size().to_int_size();
    let mut pixmap = resvg::tiny_skia::Pixmap::new(size.width(), size.height())
        .context("allocate dashboard pixmap")?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::default(),
        &mut pixmap.as_mut(),
    );
    pixmap.encode_png().context("encode dashboard png")
}
