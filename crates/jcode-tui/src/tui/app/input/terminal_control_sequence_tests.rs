use std::time::{Duration, Instant};

use super::osc_color_scrub::strip_osc_color_replies;
use super::{
    handle_text_input, insert_input_text, strip_terminal_control_sequences, text_input_for_key,
};
use crate::tui::app::tests::create_test_app;
use crossterm::event::{KeyCode, KeyModifiers};

/// Remnants of terminal reports must never reach the composer (#540).
#[test]
fn strips_escape_and_bare_report_remnants() {
    for (input, expected) in [
        // Full mouse report, and the bare tail left by a torn read.
        ("\x1b[<65;50;24M", ""),
        ("[<65;50;24M", ""),
        ("hi[<65;50;24Mthere", "hithere"),
        ("[<65;50;24m", ""),
        // Bracketed paste markers and cursor/focus reports.
        ("[200~", ""),
        ("[201~", ""),
        ("[12;40R", ""),
        ("[1I", ""),
        ("[1O", ""),
        // 8-bit CSI introducer.
        ("\u{9b}[<65;50;24M", ""),
        // Stray C0 controls, but tabs and newlines survive.
        ("a\x07b", "ab"),
        ("a\tb\nc", "a\tb\nc"),
        // Truncated escape with no final byte: drop the remnant.
        ("\x1b[<65;5", ""),
    ] {
        assert_eq!(
            strip_terminal_control_sequences(input),
            expected,
            "input {input:?} should sanitize to {expected:?}"
        );
    }
}

/// The guard must not eat text a user actually typed. Being too aggressive
/// here is worse than missing a remnant.
#[test]
fn preserves_ordinary_bracketed_text() {
    for input in [
        "array[0]",
        "list[1] = list[2]",
        "[TODO] fix this",
        "see docs[1] and notes[2]",
        "fn f(v: Vec<u8>) -> [u8; 4]",
        "a[b]c",
        "[]",
        "[",
        "]",
        "[abc]",
        "[1]",
        "[12;40]",
        "plain text with no brackets",
        "emoji 🎉 and accents café",
        "match x { [a, b] => a + b }",
    ] {
        assert_eq!(
            strip_terminal_control_sequences(input),
            input,
            "input {input:?} must be preserved verbatim"
        );
    }
}

/// Late and torn OSC 4 / 10..=19 color replies in the composer (#970, #2044).
#[test]
fn strips_late_osc_color_replies() {
    let strip_with = |input: &str, seam: Option<usize>| {
        strip_osc_color_replies(input, input.len(), seam).map(|(text, cursor)| {
            assert_eq!(cursor, text.len(), "cursor must stay at the end");
            text
        })
    };
    let strip = |input: &str| strip_with(input, None);
    let strip_seam0 = |input: &str| strip_with(input, Some(0));
    assert_eq!(strip("11;rgb:3030/3434/4646").as_deref(), Some(""));
    assert_eq!(strip("hi11;rgb:3030/3434/4646").as_deref(), Some("hi"));
    assert_eq!(
        strip("привет11;rgb:3030/3434/4646").as_deref(),
        Some("привет")
    );
    assert_eq!(strip("]11;rgb:30/34/46\\hello").as_deref(), Some("hello"));
    assert_eq!(
        strip("10;rgb:cdcd/d6d6/f4f411;rgb:0000/0000/0000").as_deref(),
        Some("")
    );
    assert_eq!(
        strip("10;rgb:3030/3434/464611;rgb:0000/0000/0000").as_deref(),
        Some("")
    );
    assert_eq!(
        strip("4;214;rgb:d7d7/afaf/5f5f4;215;rgb:d7d7/afaf/8787").as_deref(),
        Some("")
    );
    assert_eq!(strip("11;rgba:ffff/ffff/ffff/ffff").as_deref(), Some(""));

    // Bare tails only strip when anchored at the active burst seam.
    for bare_tail in ["d7/afaf/afaf\\", "/8787\\", "00\\", "87\\", "00\\87\\"] {
        assert_eq!(strip(bare_tail), None, "unprimed {bare_tail:?} must stay");
        assert_eq!(strip_seam0(bare_tail).as_deref(), Some(""));
    }
    assert_eq!(
        strip_with("draft00\\87\\", Some("draft".len())).as_deref(),
        Some("draft")
    );
    assert_eq!(strip("gb:d7/afaf/afaf\\").as_deref(), Some(""));
    assert_eq!(strip("gb:d7d7/afaf/afaf\\").as_deref(), Some(""));
    assert_eq!(strip("gb:d7d7/afaf/8787").as_deref(), Some(""));
    assert_eq!(strip("gba:ffff/ffff/ffff/ffff").as_deref(), Some(""));
    assert_eq!(strip("rgb:d7d7/afaf/afaf\\").as_deref(), Some(""));
    assert_eq!(strip(";rgb:d7d7/afaf/afaf\\").as_deref(), Some(""));
    assert_eq!(strip("1;rgb:d7d7/afaf/afaf\\").as_deref(), Some(""));
    assert_eq!(strip("4;215;rgb:d7d7/afaf/8787\\").as_deref(), Some(""));
    assert_eq!(strip("hi4;215;rgb:d7d7/afaf/8787").as_deref(), Some("hi"));
    assert_eq!(strip(";215;rgb:d7d7/afaf/8787\\").as_deref(), Some(""));
    assert_eq!(strip("215;rgb:d7d7/afaf/8787\\").as_deref(), Some(""));
    assert_eq!(strip("gb:00\\87\\").as_deref(), Some(""));
    assert_eq!(strip("gb:\\").as_deref(), Some(""));
    assert_eq!(strip("]gb:\\").as_deref(), Some(""));
    assert_eq!(strip("draft gb:\\").as_deref(), Some("draft "));
    assert_eq!(
        strip("draft gb:d7/afaf/afaf\\/8787\\00\\87\\").as_deref(),
        Some("draft ")
    );

    // Half-arrived replies are left alone until the last component is complete.
    for partial in ["11;rgb:", "11;rgb:3030/34", "11;rgb:3030/3434/46", "gb:"] {
        assert_eq!(strip(partial), None, "{partial:?}");
    }
    // Ordinary prose, identifiers, UTF-8 words, and paths survive.
    for text in [
        "rgb:3030/3434/4646",
        "rgb: ",
        "gb: ",
        "gb: something",
        "size in gb: 16",
        "en-gb: locale",
        "en-gb:\\",
        "argb:ffff/ffff/ffff\\",
        "foo_rgb:ffff/ffff/ffff\\",
        "foo-rgb:ffff/ffff/ffff\\",
        "тестrgb:ffff/ffff/ffff\\",
        "111;rgb:30/34/46",
        "use rgb(1,2,3)",
        "d7/afaf/afaf",
        "d7/beef/cafe\\",
        "/path/8787\\file",
        "./8787\\file",
        "~/8787\\file",
        "C:\\foo00\\",
        "C:\\ab\\",
        "C:\\ab\\file",
        "C:\\87\\file",
        "C:\\00\\87\\file",
        "C:\\папка00\\file",
        "printf(\"10\\n\")",
        "fix rgb in /2020",
    ] {
        assert_eq!(strip(text), None, "{text:?}");
        assert_eq!(strip_seam0(text), None, "seam0 {text:?}");
    }

    // A cursor in the middle of the draft is remapped across the removal.
    let input = "ab11;rgb:30/34/46cd";
    assert_eq!(
        strip_osc_color_replies(input, input.len() - 2, None),
        Some(("abcd".to_string(), 2))
    );
}

#[test]
fn scrubs_key_by_key_osc_bursts_and_alt_backslash_st_in_app() {
    assert_eq!(
        text_input_for_key(KeyCode::Char('\\'), KeyModifiers::ALT).as_deref(),
        Some("\\")
    );

    for initial_draft in ["", "hello ", "привет ", "🎉"] {
        for stream in [
            "gb:d7d7/afaf/8787\\",
            "4;215;rgb:d7d7/afaf/8787\\",
            "11;rgb:0000/0000/0000\\10;rgb:e6e6/eded/f3f3\\",
            "gb:d7d7/afaf/8787\\4;215;rgb:d7d7/afaf/8787\\00\\87\\",
            "11;rgb:d7/af/afd7/afaf/afaf\\/8787\\00\\87\\",
            "11;rgb:0000/0000/0000\\gb:d7d7/afaf/8787",
        ] {
            let mut app = create_test_app();
            insert_input_text(&mut app, initial_draft);
            for ch in stream.chars() {
                handle_text_input(&mut app, &ch.to_string());
            }
            assert_eq!(
                app.input, initial_draft,
                "draft {initial_draft:?} + stream {stream:?} left {0:?}",
                app.input
            );
            assert_eq!(app.cursor_pos, initial_draft.len());
        }
    }

    // No-space drafts (including hex-ending and UTF-8 drafts) preserve burst
    // seam continuations (`gb:...` and `00\`) as well as `Alt+\` ST swallowing.
    for initial_draft in ["commit deadbeef", "draft", "ab", "привет"] {
        for stream in [
            "4;215;rgb:d7d7/afaf/8787\\",
            "11;rgb:0000/0000/0000\\10;rgb:e6e6/eded/f3f3\\",
            "11;rgb:0000/0000/0000\\gb:d7d7/afaf/8787\\",
            "11;rgb:d7d7/afaf/8787\\00\\",
        ] {
            let mut app = create_test_app();
            insert_input_text(&mut app, initial_draft);
            for ch in stream.chars() {
                handle_text_input(&mut app, &ch.to_string());
            }
            assert_eq!(
                app.input, initial_draft,
                "no-space {initial_draft:?} + {stream:?}"
            );
            assert_eq!(app.cursor_pos, initial_draft.len());
        }
    }

    // Ordinary prose, paths, escapes, and UTF-8 identifiers survive key-by-key
    // typing and paste, both unprimed and right after an OSC scrub.
    for text in [
        "C:\\ab\\file",
        "C:\\ab\\",
        "ab\\file",
        "C:\\87\\file",
        "C:\\00\\87\\file",
        "D:\\d7\\afaf\\8787\\main.rs",
        "printf(\"10\\n\")",
        "gb: something",
        "size in gb: 16",
        "en-gb: locale",
        "./8787\\file",
        "~/8787\\file",
        "тестrgb:ffff/ffff/ffff\\",
        "foo_rgb:ffff/ffff/ffff\\",
        "C:\\папка00\\file",
    ] {
        let mut typed = create_test_app();
        for ch in text.chars() {
            handle_text_input(&mut typed, &ch.to_string());
        }
        assert_eq!(typed.input, text, "typed {text:?}");

        let mut pasted = create_test_app();
        handle_text_input(&mut pasted, text);
        assert_eq!(pasted.input, text, "pasted {text:?}");

        if !matches!(text, "ab\\file") {
            let mut after_osc = create_test_app();
            handle_text_input(&mut after_osc, "11;rgb:0000/0000/0000\\");
            for ch in text.chars() {
                handle_text_input(&mut after_osc, &ch.to_string());
            }
            assert_eq!(after_osc.input, text, "after_osc {text:?}");
        }
    }

    // Chunked reply on a no-space draft ending with a 2-hex-digit tail prefix
    // before its `\` arrives on the next key keeps the seam at `"draft"`.
    for draft in ["draft ", "draft"] {
        let mut app = create_test_app();
        insert_input_text(&mut app, draft);
        handle_text_input(&mut app, "11;rgb:d7d7/afaf/8787\\00");
        handle_text_input(&mut app, "\\");
        assert_eq!(app.input, draft);
        assert_eq!(app.cursor_pos, draft.len());
    }

    // Already-terminated (BEL) chunks and expired (>500ms) bursts do not eat `\`.
    let mut bel = create_test_app();
    handle_text_input(&mut bel, "11;rgb:0000/0000/0000\x07");
    handle_text_input(&mut bel, "\\");
    assert_eq!(bel.input, "\\");

    let mut expired = create_test_app();
    handle_text_input(&mut expired, "11;rgb:0000/0000/0000");
    assert_eq!(expired.input, "");
    expired.osc_scrub.scrubbed_at = Some(Instant::now() - Duration::from_secs(1));
    handle_text_input(&mut expired, "\\");
    assert_eq!(expired.input, "\\");

    // Recalling a different draft (including same-length `ab` -> `C:`) or moving
    // the cursor invalidates pending `Alt+\` ST swallowing within the 500ms window.
    for initial in ["", "ab"] {
        let mut recalled = create_test_app();
        insert_input_text(&mut recalled, initial);
        handle_text_input(&mut recalled, "11;rgb:0000/0000/0000");
        assert_eq!(recalled.input, initial);
        recalled.input = "C:".to_string();
        recalled.cursor_pos = 2;
        handle_text_input(&mut recalled, "\\");
        assert_eq!(recalled.input, "C:\\");
    }

    let mut moved = create_test_app();
    insert_input_text(&mut moved, "abcd");
    handle_text_input(&mut moved, "11;rgb:0000/0000/0000");
    moved.cursor_pos = 2;
    handle_text_input(&mut moved, "\\");
    assert_eq!(moved.input, "ab\\cd");

    // Mid-draft cursors (ASCII, Cyrillic, emoji) scrub without eating suffix text.
    for (draft, split) in [("abcd", 2), ("привет", "при".len()), ("🎉🎉", "🎉".len())]
    {
        let mut mid = create_test_app();
        insert_input_text(&mut mid, draft);
        mid.cursor_pos = split;
        for ch in "11;rgb:3030/3434/4646\\gb:d7d7/afaf/8787\\00\\".chars() {
            handle_text_input(&mut mid, &ch.to_string());
        }
        assert_eq!(mid.input, draft);
        assert_eq!(mid.cursor_pos, split);
    }
}

/// Non-suspicious text must not be reallocated.
#[test]
fn borrows_when_nothing_to_strip() {
    assert!(matches!(
        strip_terminal_control_sequences("array[0] = 1"),
        std::borrow::Cow::Borrowed(_)
    ));
}
