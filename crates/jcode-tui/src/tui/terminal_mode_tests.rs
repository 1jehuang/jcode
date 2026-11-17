//! Wire-level tests for the Kitty keyboard / tmux extended-key sequences.
//!
//! Windows reads key input through `ReadConsoleInputW`, so jcode deliberately
//! negotiates no CSI-u reporting there: `keyboard_protocol::write_mode` returns
//! `Unsupported` for a push and writes nothing for set/pop (see that module's doc
//! for #1064 / #1527 / #1812). The tests below assert the sequences a CSI-u-capable
//! terminal receives, so they are scoped to the platforms that use that decoder.
//! The Windows half is covered by `keyboard_protocol::tests`, which parameterizes
//! the decoder choice instead.
use super::{
    kitty_keyboard_push_active, reapply_terminal_modes_to,
    reset_keyboard_enhancement_ownership_for_test,
};
// Only the CSI-u byte tests below reach these; see the module doc.
#[cfg(not(windows))]
use super::{
    disable_keyboard_enhancement_to, enable_keyboard_enhancement_to,
    reapply_keyboard_enhancement_to, reapply_terminal_modes_after_focus_to,
};

#[test]
fn disable_skips_kitty_pop_when_no_push_is_owned() {
    let _guard = super::TERMINAL_OWNERSHIP_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    reset_keyboard_enhancement_ownership_for_test();

    // Greptile #4228191597: suspend_terminal_for_editor already popped our
    // entry while the editor runs, so an extra pop (signal handler firing in
    // that window) must be a no-op instead of removing the editor's own
    // stack entry.
    assert!(!super::disable_keyboard_enhancement_if_owned());
    assert!(!kitty_keyboard_push_active().load(std::sync::atomic::Ordering::Acquire));

    reset_keyboard_enhancement_ownership_for_test();
}

#[test]
#[cfg(not(windows))]
fn disable_pops_kitty_entry_exactly_once_then_ownership_is_released() {
    let _guard = super::TERMINAL_OWNERSHIP_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    reset_keyboard_enhancement_ownership_for_test();

    // Greptile #4228894633: run the pop against a buffer so the test never
    // writes the pop (or the tmux extended-keys reset) to the developer's
    // real terminal.
    kitty_keyboard_push_active().store(true, std::sync::atomic::Ordering::Release);
    let mut output = Vec::new();
    assert!(super::disable_keyboard_enhancement_if_owned_to(
        &mut output,
        false
    ));
    assert!(!output.is_empty());
    assert!(!kitty_keyboard_push_active().load(std::sync::atomic::Ordering::Acquire));

    // A second cleanup (SIGTERM during the editor window, or a duplicated
    // teardown) must not pop again now that the ownership is released.
    let mut second = Vec::new();
    assert!(!super::disable_keyboard_enhancement_if_owned_to(
        &mut second,
        false
    ));
    assert!(second.is_empty());

    reset_keyboard_enhancement_ownership_for_test();
}

#[test]
#[cfg(not(windows))]
fn adopt_records_inherited_entry_without_pushing() {
    let _guard = super::TERMINAL_OWNERSHIP_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    reset_keyboard_enhancement_ownership_for_test();

    // Greptile #4228894613: after an exec handoff the entry is already on
    // the stack, so adopting must only record ownership — no push bytes —
    // and the pop must then balance the inherited entry.
    super::adopt_keyboard_enhancement_ownership();
    assert!(kitty_keyboard_push_active().load(std::sync::atomic::Ordering::Acquire));

    let mut output = Vec::new();
    assert!(super::disable_keyboard_enhancement_if_owned_to(
        &mut output,
        false
    ));
    assert_eq!(output, b"\x1b[<1u");

    reset_keyboard_enhancement_ownership_for_test();
}

#[test]
fn cleanup_waits_for_in_flight_kitty_push_to_finish() {
    let _guard = super::TERMINAL_OWNERSHIP_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    reset_keyboard_enhancement_ownership_for_test();

    // Greptile #4228894626: a termination cleanup must not observe the
    // half-applied state between the push write and the ownership flag,
    // so the disable waits for the state lock instead of racing it. Hold
    // the lock, set the flag as if a push just completed, and confirm a
    // concurrent cleanup stays blocked until the lock is released.
    let state_guard = super::kitty_keyboard_state_lock()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    kitty_keyboard_push_active().store(true, std::sync::atomic::Ordering::Release);

    let cleanup = std::thread::spawn(|| {
        // Buffer-backed so the blocked cleanup writes no bytes to the
        // developer's terminal; the flag swap is the observable effect.
        let mut sink = Vec::new();
        super::disable_keyboard_enhancement_if_owned_to(&mut sink, false)
    });
    std::thread::sleep(std::time::Duration::from_millis(100));
    // The cleanup is still blocked on the state lock we hold, so the flag
    // must be untouched and the thread must not have returned yet.
    assert!(!cleanup.is_finished());
    assert!(kitty_keyboard_push_active().load(std::sync::atomic::Ordering::Acquire));

    drop(state_guard);
    assert!(cleanup.join().expect("cleanup thread"));
    assert!(!kitty_keyboard_push_active().load(std::sync::atomic::Ordering::Acquire));

    reset_keyboard_enhancement_ownership_for_test();
}

#[test]
#[cfg(not(windows))]
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
#[cfg(not(windows))]
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
#[cfg(not(windows))]
fn reapply_omits_mouse_sequences_when_capture_is_disabled() {
    let mut output = Vec::new();
    reapply_terminal_modes_to(&mut output, false, true, true).unwrap();

    let output = String::from_utf8(output).unwrap();
    assert!(output.starts_with("\x1b[?2004h\x1b[?1004h"));
    assert!(!output.contains("\x1b[?1000h"));
    assert!(output.contains("\x1b[="));
}

#[test]
#[cfg(not(windows))]
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
#[cfg(not(windows))]
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
