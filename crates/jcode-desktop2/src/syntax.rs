//! Syntax colours for code, from the file's own extension.
//!
//! A diff is read by scanning: which side a line is on, and *what* the line
//! says. Colouring the whole line one ink answers the first question and
//! destroys the second, so a hundred-line rename reads as two solid blocks of
//! green and red with no structure inside them. Syntax highlighting puts the
//! structure back: a string stays a string on both sides of the change, and the
//! eye lands on the token that actually differs.
//!
//! Highlighting is done per line rather than per file, because that is what the
//! app has: an edit tool reports the lines it touched, not the file around them.
//! A line-local highlighter cannot know it is inside a block comment, which is
//! the one thing this trades away; carrying whole-file state for a two-line
//! diff would mean reading the file back from disk on every frame.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::{Mutex, OnceLock};
use syntect::easy::HighlightLines;
use syntect::highlighting::{Theme as SyntectTheme, ThemeSet};
use syntect::parsing::SyntaxSet;
use vello::peniko::Color;

/// One highlighted run: a byte range within the line, and the ink for it.
pub type Run = (Range<usize>, Color);

fn syntaxes() -> &'static SyntaxSet {
    static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();
    SYNTAXES.get_or_init(SyntaxSet::load_defaults_newlines)
}

/// The syntect theme matching the app's own mode. Two themes, not one: a
/// palette tuned for a dark editor is unreadable on paper, and vice versa.
fn theme(dark: bool) -> &'static SyntectTheme {
    static THEMES: OnceLock<(SyntectTheme, SyntectTheme)> = OnceLock::new();
    let (light, dark_theme) = THEMES.get_or_init(|| {
        let set = ThemeSet::load_defaults();
        (
            set.themes["InspiredGitHub"].clone(),
            set.themes["base16-ocean.dark"].clone(),
        )
    });
    if dark { dark_theme } else { light }
}

/// Highlighted runs for one line of `language`-flavoured code.
///
/// `language` is a file extension (`rs`), a fence token (`rust`), or `None`
/// when nothing said. An unknown language yields no runs at all, which the
/// caller renders in its own ink: unknown syntax must render as ordinary code,
/// never as an error and never as a wall of one accent.
///
/// Results are memoised per (line, language, mode): the transcript re-lays a
/// message whenever the window resizes or a delta arrives, and re-running a
/// regex-based highlighter over an unchanged diff every time is the kind of
/// cost that grows with the length of the session.
pub fn highlight_line(line: &str, language: Option<&str>, dark: bool) -> Vec<Run> {
    if line.trim().is_empty() || language.is_none() {
        return Vec::new();
    }
    type Cache = HashMap<(String, String, bool), Vec<Run>>;
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let key = (
        line.to_string(),
        language.unwrap_or_default().to_string(),
        dark,
    );
    if let Ok(cache) = cache.lock()
        && let Some(runs) = cache.get(&key)
    {
        return runs.clone();
    }
    let runs = highlight_uncached(line, language, dark);
    if let Ok(mut cache) = cache.lock() {
        // A session can touch a lot of distinct lines; the cache is a speedup,
        // not a store, so it is dropped wholesale rather than grown without
        // bound. Clearing beats evicting one entry: the working set of a
        // conversation is the diff on screen, which is re-highlighted in a
        // frame or two anyway.
        if cache.len() > CACHE_LIMIT {
            cache.clear();
        }
        cache.insert(key, runs.clone());
    }
    runs
}

const CACHE_LIMIT: usize = 4096;

fn highlight_uncached(line: &str, language: Option<&str>, dark: bool) -> Vec<Run> {
    let syntaxes = syntaxes();
    let Some(syntax) = language.and_then(|language| {
        syntaxes
            .find_syntax_by_extension(language)
            .or_else(|| syntaxes.find_syntax_by_token(language))
    }) else {
        return Vec::new();
    };
    let mut highlighter = HighlightLines::new(syntax, theme(dark));
    let Ok(ranges) = highlighter.highlight_line(line, syntaxes) else {
        return Vec::new();
    };
    let mut runs = Vec::new();
    let mut at = 0usize;
    for (style, text) in ranges {
        let range = at..at + text.len();
        at = range.end;
        if text.trim().is_empty() {
            continue;
        }
        let color = style.foreground;
        runs.push((range, Color::from_rgb8(color.r, color.g, color.b)));
    }
    runs
}

/// The language token for a path, from its extension. `None` when the file has
/// no extension, which is the honest answer: guessing `sh` for a bare
/// `Makefile` colours it wrongly and confidently.
pub fn language_for(path: &str) -> Option<&str> {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    name.rsplit_once('.')
        .map(|(_, extension)| extension)
        .filter(|extension| !extension.is_empty())
}

/// Blend `ink` toward `toward`, so a highlighted token still reads as belonging
/// to the side of the diff it is on.
///
/// The mix is deliberately gentle: pulled all the way to the diff colour, the
/// syntax is gone again, and left alone the two sides stop being scannable.
pub fn tint(ink: Color, toward: Color, amount: f64) -> Color {
    let [r, g, b, a] = ink.components;
    let [tr, tg, tb, _] = toward.components;
    let amount = amount.clamp(0.0, 1.0) as f32;
    let mix = |from: f32, to: f32| from + (to - from) * amount;
    Color::new([mix(r, tr), mix(g, tg), mix(b, tb), a])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A known language is coloured, and the runs tile the line in order: an
    /// overlapping or out-of-order run would paint one token's ink over
    /// another's when the spans are applied.
    #[test]
    fn a_known_language_is_coloured_in_order() {
        let line = "let x = \"hi\";";
        let runs = highlight_line(line, Some("rs"), true);
        assert!(!runs.is_empty(), "rust should highlight");
        let mut end = 0;
        for (range, _) in &runs {
            assert!(range.start >= end, "runs overlap or are unordered");
            assert!(range.end <= line.len());
            end = range.end;
        }
        // A keyword and a string literal must not end up the same ink, or the
        // highlighting is decorative rather than informative.
        let inks: Vec<_> = runs.iter().map(|(_, color)| color.components).collect();
        assert!(inks.windows(2).any(|pair| pair[0] != pair[1]));
    }

    /// Unknown or absent languages render as plain code, not as an error and
    /// not as a panic.
    #[test]
    fn an_unknown_language_is_plain() {
        assert!(highlight_line("some prose", None, true).is_empty());
        assert!(highlight_line("some prose", Some("zzzz"), true).is_empty());
        assert!(highlight_line("", Some("rs"), true).is_empty());
    }

    #[test]
    fn a_language_comes_from_the_extension() {
        assert_eq!(language_for("src/main.rs"), Some("rs"));
        assert_eq!(language_for("a/b/Makefile"), None);
        assert_eq!(language_for("x.tar.gz"), Some("gz"));
    }

    /// Tinting moves ink toward the target without reaching it, so both the
    /// token's identity and the side it is on survive.
    #[test]
    fn a_tint_keeps_both_signals() {
        let ink = Color::from_rgb8(0x40, 0x80, 0xc0);
        let toward = Color::from_rgb8(0x00, 0xff, 0x00);
        let tinted = tint(ink, toward, 0.3);
        assert_ne!(tinted.components, ink.components);
        assert_ne!(tinted.components, toward.components);
    }

    /// The same line asked for twice is the same answer: the cache must not
    /// change what is drawn, only how long it took.
    #[test]
    fn the_cache_returns_the_same_colours() {
        let first = highlight_line("fn main() {}", Some("rs"), false);
        let second = highlight_line("fn main() {}", Some("rs"), false);
        assert_eq!(first, second);
    }
}
