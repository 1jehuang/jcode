//! Tests for `#[path]`-attributed module `terminal_mode_tests` of `mod.rs`.
//!
//! Extracted from the parent file: an inline `#[cfg(test)] mod` counts toward
//! the code-size budget, which only exempts `*_tests.rs` files.

use super::{
    disable_keyboard_enhancement_to, enable_keyboard_enhancement_to,
    reapply_keyboard_enhancement_to, reapply_terminal_modes_after_focus_to,
    reapply_terminal_modes_to,
};

#[test]
fn tmux_keyboard_lifecycle_requests_and_resets_extended_keys() {
    let mut output = Vec::new();
    enable_keyboard_enhancement_to(&mut output, true).unwrap();
    assert_eq!(output, b"\x1b[>4;2m\x1b[>7u");

    output.clear();
    reapply_keyboard_enhancement_to(&mut output, true).unwrap();
    assert_eq!(output, b"\x1b[>4;2m\x1b[=7u");

    output.clear();
    disable_keyboard_enhancement_to(&mut output, true).unwrap();
    assert_eq!(output, b"\x1b[>4;0m\x1b[<1u");
}

#[test]
fn outside_tmux_keyboard_lifecycle_keeps_kitty_protocol_only() {
    let mut output = Vec::new();
    enable_keyboard_enhancement_to(&mut output, false).unwrap();
    assert_eq!(output, b"\x1b[>7u");

    output.clear();
    reapply_keyboard_enhancement_to(&mut output, false).unwrap();
    assert_eq!(output, b"\x1b[=7u");

    output.clear();
    disable_keyboard_enhancement_to(&mut output, false).unwrap();
    assert_eq!(output, b"\x1b[<1u");
}

#[test]
fn reapply_omits_keyboard_protocols_when_disabled() {
    let mut output = Vec::new();
    reapply_terminal_modes_to(&mut output, false, false, false).unwrap();
    assert_eq!(output, b"\x1b[?2004h");
}

#[test]
fn reapply_omits_mouse_sequences_when_capture_is_disabled() {
    let mut output = Vec::new();
    reapply_terminal_modes_to(&mut output, false, true, true).unwrap();

    let output = String::from_utf8(output).unwrap();
    assert!(output.starts_with("\x1b[?2004h\x1b[?1004h"));
    assert!(!output.contains("\x1b[?1000h"));
    assert!(output.contains("\x1b[="));
}

#[test]
fn reapply_emits_configured_idempotent_modes_without_keyboard_push() {
    let mut output = Vec::new();
    reapply_terminal_modes_to(&mut output, true, true, true).unwrap();

    let output = String::from_utf8(output).unwrap();
    assert!(output.contains("\x1b[?2004h"));
    assert!(output.contains("\x1b[?1004h"));
    assert!(output.contains("\x1b[?1000h"));
    assert!(output.contains("\x1b[="), "must set Kitty keyboard flags");
    assert!(
        !output.contains("\x1b[>7u"),
        "must not push the Kitty keyboard stack"
    );
}

#[test]
fn focus_reapply_preserves_other_modes_without_rearming_focus_reporting() {
    for mouse_capture in [false, true] {
        for keyboard_enhanced in [false, true] {
            let mut output = Vec::new();
            reapply_terminal_modes_after_focus_to(&mut output, mouse_capture, keyboard_enhanced)
                .unwrap();

            let output = String::from_utf8(output).unwrap();
            assert!(output.starts_with("\x1b[?2004h"));
            assert!(
                !output.contains("\x1b[?1004h"),
                "must not trigger a focus reply"
            );
            assert!(
                !output.contains("\x1b[?1004l"),
                "must keep focus reporting enabled"
            );
            assert_eq!(output.contains("\x1b[?1000h"), mouse_capture);
            assert_eq!(output.contains("\x1b[=7u"), keyboard_enhanced);
            assert!(
                !output.contains("\x1b[>7u"),
                "must not push the Kitty keyboard stack (tmux modifyOtherKeys is allowed)"
            );
        }
    }
}
