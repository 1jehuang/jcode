//! Registries that map text-rendered math (Unicode fallback) back to the LaTeX
//! source it came from, so copying rendered math yields LaTeX instead of the
//! terminal-friendly approximation that is displayed.

use ratatui::prelude::Line;
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, LazyLock, Mutex};

const DISPLAY_LIMIT: usize = 4096;
const INLINE_LIMIT: usize = 8192;
const LINE_LIMIT: usize = 16384;

struct Bounded<V> {
    entries: HashMap<u64, V>,
    order: VecDeque<u64>,
    limit: usize,
}

impl<V> Bounded<V> {
    fn new(limit: usize) -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
            limit,
        }
    }

    fn insert(&mut self, key: u64, value: V) {
        if self.entries.insert(key, value).is_none() {
            self.order.push_back(key);
        }
        while self.order.len() > self.limit {
            if let Some(oldest) = self.order.pop_front() {
                self.entries.remove(&oldest);
            }
        }
    }
}

static DISPLAY_SOURCES: LazyLock<Mutex<Bounded<String>>> =
    LazyLock::new(|| Mutex::new(Bounded::new(DISPLAY_LIMIT)));
static INLINE_SOURCES: LazyLock<Mutex<Bounded<String>>> =
    LazyLock::new(|| Mutex::new(Bounded::new(INLINE_LIMIT)));
static LINE_SPANS: LazyLock<Mutex<Bounded<Arc<Vec<InlineMathSpan>>>>> =
    LazyLock::new(|| Mutex::new(Bounded::new(LINE_LIMIT)));

fn hash_str(value: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

/// Display-math bodies may be re-wrapped or re-indented (centering, narrow
/// widths) between render and copy, so key on the non-whitespace content only.
fn display_key<'a>(lines: impl IntoIterator<Item = &'a str>) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for line in lines {
        for ch in line.chars().filter(|ch| !ch.is_whitespace()) {
            ch.hash(&mut hasher);
        }
    }
    hasher.finish()
}

pub(crate) fn register_display_math(rendered_lines: &[String], source: &str) {
    let key = display_key(rendered_lines.iter().map(String::as_str));
    if let Ok(mut cache) = DISPLAY_SOURCES.lock() {
        cache.insert(key, source.trim().to_string());
    }
}

/// LaTeX source (without delimiters) for a Unicode display-math frame body.
pub(crate) fn display_math_source(content_lines: &[String]) -> Option<String> {
    let key = display_key(content_lines.iter().map(String::as_str));
    DISPLAY_SOURCES
        .lock()
        .ok()
        .and_then(|cache| cache.entries.get(&key).cloned())
}

pub(crate) fn register_inline_math(rendered: &str, source: &str) {
    if rendered.is_empty() {
        return;
    }
    if let Ok(mut cache) = INLINE_SOURCES.lock() {
        cache.insert(hash_str(rendered), source.trim().to_string());
    }
}

/// An inline math span inside a rendered plain-text line, in display columns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InlineMathSpan {
    pub start_col: usize,
    pub end_col: usize,
    /// LaTeX source without `$` delimiters.
    pub source: String,
}

/// Locate inline math spans in a rendered line. Math spans are identified by
/// their dedicated inline-math style plus a registered rendered->source entry,
/// so ordinary prose that happens to equal a rendered formula is not matched.
pub fn inline_math_spans(line: &Line<'_>) -> Vec<InlineMathSpan> {
    let math_fg = Some(super::math_inline_fg());
    let mut spans = Vec::new();
    let mut col = 0usize;
    let cache = INLINE_SOURCES.lock().ok();
    for span in &line.spans {
        let width = unicode_width::UnicodeWidthStr::width(span.content.as_ref());
        if span.style.fg == math_fg
            && let Some(cache) = cache.as_ref()
            && let Some(source) = cache.entries.get(&hash_str(span.content.as_ref()))
        {
            spans.push(InlineMathSpan {
                start_col: col,
                end_col: col + width,
                source: source.clone(),
            });
        }
        col += width;
    }
    spans
}

/// Remember the inline math spans of a rendered logical line, keyed by its
/// plain text, so text selection over that line can copy LaTeX source.
pub fn record_inline_math_for_line(line: &Line<'_>) {
    let math_fg = Some(super::math_inline_fg());
    if !line.spans.iter().any(|span| span.style.fg == math_fg) {
        return;
    }
    let spans = inline_math_spans(line);
    if spans.is_empty() {
        return;
    }
    let plain: String = line
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect();
    if let Ok(mut cache) = LINE_SPANS.lock() {
        cache.insert(hash_str(&plain), Arc::new(spans));
    }
}

/// Inline math spans previously recorded for a plain logical line.
pub fn inline_math_spans_for_plain_line(plain: &str) -> Option<Arc<Vec<InlineMathSpan>>> {
    if plain.is_empty() {
        return None;
    }
    LINE_SPANS
        .lock()
        .ok()
        .and_then(|cache| cache.entries.get(&hash_str(plain)).cloned())
}
