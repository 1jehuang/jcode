mod replay {
    use super::super::super::KITTY_KEYBOARD_PUSH_ACTIVE;
    use super::*;

    #[test]
    fn typing_right_after_a_completed_report_is_replayed() {
        let _lock = guard_test_lock();
        // `2x` queued behind a burst: the digit alone is not a report
        // continuation (a second report starts with its own `[`), and the
        // final `x` is not a report final, so both characters must reach
        // the app instead of being swallowed with the report.
        let mut guard = CsiTailGuard::default();
        let now = Instant::now();
        let delivered = feed_guard_split_tail(&mut guard, escape(), b"[O", now);
        assert_eq!(delivered, vec![Event::FocusLost]);
        KITTY_KEYBOARD_PUSH_ACTIVE.store(true, Ordering::Relaxed);
        let outcome = guard.push(press('2'), now + Duration::from_millis(3));
        assert!(matches!(outcome, GuardOutcome::Held));
        let outcome = guard.push(press('x'), now + Duration::from_millis(4));
        KITTY_KEYBOARD_PUSH_ACTIVE.store(false, Ordering::Relaxed);
        let delivered = match outcome {
            GuardOutcome::Pass(events) => events,
            other => panic!("expected pass, got {other:?}"),
        };
        assert_eq!(delivered, vec![press('2'), press('x')]);
    }

    #[test]
    fn headless_tail_without_separator_replays_typing() {
        let _lock = guard_test_lock();
        // Fast `1M` after a report matches the grammar but lacks a separator.
        // It must replay, unlike a headless mouse tail such as `35;49;28M`.
        let mut guard = CsiTailGuard::default();
        let now = Instant::now();
        let mut delivered = feed_guard_split_tail(&mut guard, escape(), b"[O", now);
        assert_eq!(delivered, vec![Event::FocusLost]);
        assert_eq!(guard.phase, CsiTailPhase::Post);
        KITTY_KEYBOARD_PUSH_ACTIVE.store(true, Ordering::Relaxed);
        let mut at = now + Duration::from_millis(3);
        assert!(matches!(guard.push(press('1'), at), GuardOutcome::Held));
        at += Duration::from_millis(1);
        let outcome = guard.push(press('M'), at);
        KITTY_KEYBOARD_PUSH_ACTIVE.store(false, Ordering::Relaxed);
        match outcome {
            GuardOutcome::Pass(events) => delivered.extend(events),
            other => panic!("expected typing to replay, got {other:?}"),
        }
        assert_eq!(delivered, vec![Event::FocusLost, press('1'), press('M')]);
    }
}
