// Invariants for the idle-animation partial repaint fast path.
//
// On an idle screen the decorative animation is the only moving element, yet
// the run loop used to render a whole frame per animation tick (~50ms median
// on an 8-core laptop, dominated by transcript/header/composer work producing
// byte-identical cells). The fast path repaints only the animation rows over
// the previous frame, so these tests pin the two properties it depends on:
//
// 1. `ui::draw` publishes the animated rectangle when (and only when) the
//    animation actually rendered, so the loop never patches stale rows.
// 2. A partial repaint of those rows reproduces the same cells a full frame
//    would have produced, so the shortcut is visually indistinguishable.

use ratatui::Terminal;
use ratatui::backend::TestBackend;

fn idle_animation_state(anim_elapsed: f32) -> TestState {
    TestState {
        anim_elapsed,
        // Empty transcript + Idle status is what `idle_donut_active` treats as
        // an idle screen, which is when the animation renders.
        ..Default::default()
    }
}

fn render_full(state: &TestState, width: u16, height: u16) -> Terminal<TestBackend> {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("test terminal");
    terminal
        .draw(|frame| crate::tui::ui::draw(frame, state))
        .expect("full frame");
    terminal
}

#[test]
fn draw_publishes_the_animated_rows_only_when_the_animation_rendered() {
    let _lock = viewport_snapshot_test_lock();
    clear_flicker_frame_history_for_tests();

    let idle = idle_animation_state(1.0);
    let terminal = render_full(&idle, 100, 40);
    let area = crate::tui::ui::last_idle_animation_area()
        .expect("an idle screen must publish the animated rectangle");
    let frame_area = *terminal.backend().buffer().area();
    assert!(area.width >= 4 && area.height >= 2, "degenerate area {area:?}");
    assert!(
        area.right() <= frame_area.right() && area.bottom() <= frame_area.bottom(),
        "animated rows {area:?} escaped the frame {frame_area:?}"
    );

    // A screen with a real conversation has no decorative animation, so the
    // fast path must be disabled rather than patching whatever rows the
    // previous idle frame animated.
    let busy = TestState {
        display_messages: vec![DisplayMessage::user("hello".to_string())],
        ..Default::default()
    };
    let _ = render_full(&busy, 100, 40);
    assert_eq!(
        crate::tui::ui::last_idle_animation_area(),
        None,
        "a conversation screen must not advertise animated rows"
    );
}

#[test]
fn partial_repaint_matches_a_full_frame_at_the_same_animation_time() {
    let _lock = viewport_snapshot_test_lock();
    clear_flicker_frame_history_for_tests();

    // Frame 1: full render at t0, which is what the run loop keeps around.
    let first = render_full(&idle_animation_state(1.0), 100, 40);
    let area = crate::tui::ui::last_idle_animation_area().expect("animated rectangle");
    let mut patched = first.backend().buffer().clone();

    // Frame 2: the reference full render at t1.
    let second = render_full(&idle_animation_state(1.5), 100, 40);
    let expected = second.backend().buffer();

    // The fast path: reuse frame 1 and repaint only the animated rows at t1.
    crate::tui::ui::render_idle_animation_into(&mut patched, area, 1.5);

    assert_eq!(
        patched.area, expected.area,
        "partial repaint changed the frame geometry"
    );
    let mismatches: Vec<_> = patched
        .content
        .iter()
        .zip(expected.content.iter())
        .enumerate()
        .filter(|(_, (left, right))| left != right)
        .map(|(idx, (l, r))| {
            (
                idx as u16 % patched.area.width,
                idx as u16 / patched.area.width,
                l.symbol().to_string(),
                r.symbol().to_string(),
            )
        })
        .collect();
    assert!(
        mismatches.is_empty(),
        "partial repaint diverged from a full frame at {} cells: {:?}",
        mismatches.len(),
        &mismatches[..mismatches.len().min(8)]
    );
}

#[test]
fn advancing_the_animation_actually_changes_the_animated_rows() {
    // Guards against the partial-repaint test passing vacuously: if the
    // animation were static, "matches a full frame" would be trivially true and
    // the fast path could silently freeze the animation.
    let _lock = viewport_snapshot_test_lock();
    clear_flicker_frame_history_for_tests();

    let first = render_full(&idle_animation_state(1.0), 100, 40);
    let area = crate::tui::ui::last_idle_animation_area().expect("animated rectangle");
    let before = first.backend().buffer().clone();
    let mut after = before.clone();
    crate::tui::ui::render_idle_animation_into(&mut after, area, 1.5);

    assert_ne!(
        before.content, after.content,
        "the animation must visibly advance between ticks"
    );

    // And it must stay inside its own rows: everything outside is reused, so a
    // stray write would corrupt the transcript/composer.
    for y in 0..before.area.height {
        for x in 0..before.area.width {
            if area.contains((x, y).into()) {
                continue;
            }
            assert_eq!(
                before[(x, y)],
                after[(x, y)],
                "partial repaint wrote outside the animated rows at ({x}, {y})"
            );
        }
    }
}

#[test]
fn idle_animation_is_excluded_from_the_full_frame_redraw_signal() {
    // `handle_tick` uses the excluding variant so an animation-only tick does
    // not force a full frame; the animation still drives the tick cadence.
    let _lock = viewport_snapshot_test_lock();
    clear_flicker_frame_history_for_tests();

    let idle = idle_animation_state(1.0);
    assert!(
        crate::tui::periodic_redraw_required(&idle),
        "the animation must keep the redraw loop ticking"
    );
    assert!(
        !crate::tui::periodic_redraw_required_excluding_idle_animation(&idle),
        "an animation-only tick must not request a full frame"
    );

    // Real activity must still request full frames.
    let busy = TestState {
        status: ProcessingStatus::Streaming,
        ..Default::default()
    };
    assert!(
        crate::tui::periodic_redraw_required_excluding_idle_animation(&busy),
        "live activity must still request full frames"
    );
}

#[test]
fn partial_repaint_does_no_layout_or_transcript_work() {
    // The savings come entirely from skipping the frame pipeline, so assert that
    // structurally rather than by timing (which varies by build profile): a full
    // frame republishes layout/viewport bookkeeping, a partial repaint must
    // touch none of it and must not resize or relayout anything.
    let _lock = viewport_snapshot_test_lock();
    clear_flicker_frame_history_for_tests();

    let state = idle_animation_state(1.0);
    let terminal = render_full(&state, 100, 40);
    let area = crate::tui::ui::last_idle_animation_area().expect("animated rectangle");
    let layout_before = crate::tui::ui::last_layout_snapshot().expect("layout snapshot");
    let status_before = crate::tui::ui::last_status_area().expect("status area");
    let mut buffer = terminal.backend().buffer().clone();

    crate::tui::ui::render_idle_animation_into(&mut buffer, area, 1.5);

    assert_eq!(
        crate::tui::ui::last_idle_animation_area(),
        Some(area),
        "a partial repaint must not disturb the published animation rectangle"
    );
    assert_eq!(
        crate::tui::ui::last_status_area(),
        Some(status_before),
        "a partial repaint must not re-run status layout"
    );
    let layout_after = crate::tui::ui::last_layout_snapshot().expect("layout snapshot");
    assert_eq!(
        layout_after.messages_area, layout_before.messages_area,
        "a partial repaint must not re-run transcript layout"
    );
    assert_eq!(
        layout_after.input_area, layout_before.input_area,
        "a partial repaint must not re-run composer layout"
    );
}

#[test]
fn published_rectangle_round_trips_through_the_shared_slot() {
    // Publishing must be an exact round trip, including the "no animation"
    // state, or the fast path would patch the wrong rows.
    let _lock = viewport_snapshot_test_lock();

    for area in [
        ratatui::layout::Rect::new(0, 0, 4, 2),
        ratatui::layout::Rect::new(7, 26, 94, 14),
        ratatui::layout::Rect::new(u16::MAX - 1, u16::MAX - 1, 1, 1),
    ] {
        crate::tui::ui::record_idle_animation_area(Some(area));
        assert_eq!(crate::tui::ui::last_idle_animation_area(), Some(area));
    }

    crate::tui::ui::record_idle_animation_area(None);
    assert_eq!(crate::tui::ui::last_idle_animation_area(), None);
}
