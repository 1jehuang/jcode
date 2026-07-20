use std::borrow::Cow;
use std::sync::atomic::{AtomicBool, Ordering};
use unicode_properties::emoji::{
    EmojiStatus, UnicodeEmoji, is_emoji_presentation_selector, is_regional_indicator,
    is_text_presentation_selector, is_zwj,
};
use unicode_segmentation::UnicodeSegmentation;

static EMOJI_ENABLED: AtomicBool = AtomicBool::new(true);

#[macro_export]
macro_rules! terminal_print {
    ($($arg:tt)*) => {{
        let message = ::std::format!($($arg)*);
        ::std::print!("{}", $crate::output_style::terminal_text(&message));
    }};
}

#[macro_export]
macro_rules! terminal_println {
    () => { ::std::println!() };
    ($($arg:tt)*) => {{
        let message = ::std::format!($($arg)*);
        ::std::println!("{}", $crate::output_style::terminal_text(&message));
    }};
}

#[macro_export]
macro_rules! terminal_eprint {
    ($($arg:tt)*) => {{
        let message = ::std::format!($($arg)*);
        ::std::eprint!("{}", $crate::output_style::terminal_text(&message));
    }};
}

#[macro_export]
macro_rules! terminal_eprintln {
    () => { ::std::eprintln!() };
    ($($arg:tt)*) => {{
        let message = ::std::format!($($arg)*);
        ::std::eprintln!("{}", $crate::output_style::terminal_text(&message));
    }};
}

/// Set whether terminal-facing output may contain emoji.
pub fn set_emoji_enabled(enabled: bool) {
    EMOJI_ENABLED.store(enabled, Ordering::Relaxed);
}

/// Return whether terminal-facing output may contain emoji.
pub fn emoji_enabled() -> bool {
    EMOJI_ENABLED.load(Ordering::Relaxed)
}

/// Adapt terminal-facing text to the configured emoji preference.
pub fn terminal_text(text: &str) -> Cow<'_, str> {
    terminal_text_with_emoji(text, emoji_enabled())
}

/// Adapt terminal-facing text using an explicit emoji preference.
pub fn terminal_text_with_emoji(text: &str, enabled: bool) -> Cow<'_, str> {
    if enabled || text.is_ascii() || !contains_emoji(text) {
        Cow::Borrowed(text)
    } else {
        Cow::Owned(replace_emoji_with_ascii(text))
    }
}

/// Replace emoji grapheme clusters with compact ASCII markers.
pub fn replace_emoji_with_ascii(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    for grapheme in text.graphemes(true) {
        if grapheme_is_emoji(grapheme) {
            output.push_str(emoji_ascii_fallback(grapheme));
        } else {
            output.push_str(grapheme);
        }
    }
    output
}

fn contains_emoji(text: &str) -> bool {
    text.graphemes(true).any(grapheme_is_emoji)
}

fn emoji_ascii_fallback(grapheme: &str) -> &'static str {
    if grapheme.chars().any(|ch| matches!(ch, '✓' | '✔' | '✅')) {
        "+"
    } else if grapheme
        .chars()
        .any(|ch| matches!(ch, '✕' | '✗' | '❌' | '❎'))
    {
        "x"
    } else if grapheme.chars().any(|ch| matches!(ch, '⚠' | '🚨')) {
        "!"
    } else if grapheme
        .chars()
        .any(|ch| matches!(ch, '➡' | '👉' | '➜' | '➤'))
    {
        "->"
    } else if grapheme.chars().any(|ch| matches!(ch, '⬅' | '👈')) {
        "<-"
    } else {
        "*"
    }
}

fn grapheme_is_emoji(grapheme: &str) -> bool {
    let has_text_selector = grapheme.chars().any(is_text_presentation_selector);
    let has_emoji_selector = grapheme.chars().any(is_emoji_presentation_selector);
    if has_text_selector && !has_emoji_selector {
        return false;
    }

    let has_emoji_char = grapheme.chars().any(UnicodeEmoji::is_emoji_char);
    let regional_indicators = grapheme
        .chars()
        .filter(|ch| is_regional_indicator(*ch))
        .count();
    has_emoji_selector
        || grapheme.contains('\u{20E3}')
        || regional_indicators >= 2
        || (has_emoji_char && grapheme.chars().any(is_zwj))
        || grapheme.chars().any(|ch| {
            matches!(
                ch.emoji_status(),
                EmojiStatus::EmojiPresentation
                    | EmojiStatus::EmojiPresentationAndModifierBase
                    | EmojiStatus::EmojiPresentationAndEmojiComponent
                    | EmojiStatus::EmojiPresentationAndModifierAndEmojiComponent
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emoji_clusters_use_readable_ascii_fallbacks() {
        assert_eq!(
            replace_emoji_with_ascii(
                "🐝 ready ✅ warning ⚠️ failed ❌ family 👨‍👩‍👧‍👦 tone 👋🏽 flag 🇺🇸 key 1️⃣"
            ),
            "* ready + warning ! failed x family * tone * flag * key *"
        );
    }

    #[test]
    fn non_emoji_unicode_is_preserved() {
        assert_eq!(
            replace_emoji_with_ascii("box ─│ arrows →←↔ CJK 中文 math α © ® ✓ ✗ ⚠"),
            "box ─│ arrows →←↔ CJK 中文 math α © ® ✓ ✗ ⚠"
        );
        assert_eq!(replace_emoji_with_ascii("text heart ♥︎"), "text heart ♥︎");
    }
}
