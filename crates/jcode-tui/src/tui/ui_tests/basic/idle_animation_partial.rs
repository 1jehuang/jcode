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
    // The animation only paces the loop when it is actually on screen, so put it
    // there first by drawing a frame. Asserting the cadence without this would
    // describe a state the run loop never reaches: nothing published means
    // nothing to animate, and ticking for it is the wasted-frame bug.
    let _ = render_full(&idle, 100, 40);
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

/// The bug this pins, reproduced from a live client: on a freshly spawned
/// session the `/resume` session picker is up, and the redraw loop ran at
/// animation cadence (60fps) while the animation-only partial repaint was
/// unavailable, so every tick became a full frame.
///
/// Measured before the fix with `scripts/count_idle_draws.py` against the real
/// binary: 63 full `terminal.draw` calls per second, each changing **0 of 7680
/// cells**, on a screen that `scripts/dump_fresh_spawn_screen.py` (a real
/// terminal emulator) confirmed was completely static. `draw-stats` reported
/// `no_animation_area` x365 over the same window, and an instrumented build
/// named the state: `session_picker=true`.
///
/// The mechanism is a disagreement between two halves of the render loop:
/// `ui::draw_inner` returns early for full-screen overlays, *before* the donut
/// chunk that publishes the animated rectangle, while the redraw scheduler was
/// pacing the loop off "does this screen want a donut?" and never noticed.
///
/// This asserts the *cadence*, which is the thing that costs CPU, rather than
/// the renderer's own decision, so any reconciliation that stops the waste
/// passes.
#[test]
fn a_full_screen_overlay_stops_the_decorative_animation_cadence() {
    let _lock = viewport_snapshot_test_lock();
    clear_flicker_frame_history_for_tests();

    // Baseline: this screen really is an animating idle screen once drawn, so a
    // pass below cannot come from the animation being off for other reasons.
    let idle = idle_animation_state(1.0);
    let _ = render_full(&idle, 160, 48);
    let animation_interval = crate::tui::redraw_interval(&idle);
    assert!(
        animation_interval < crate::tui::REDRAW_IDLE,
        "baseline idle screen should tick faster than the idle cadence, got \
         {animation_interval:?}"
    );

    let overlays: Vec<(&str, TestState)> = vec![
        (
            "changelog",
            TestState {
                changelog_scroll: Some(0),
                ..idle_animation_state(1.0)
            },
        ),
        (
            "help",
            TestState {
                help_scroll: Some(0),
                ..idle_animation_state(1.0)
            },
        ),
    ];

    for (name, state) in overlays {
        // Render the overlay: this is the frame that (correctly) publishes no
        // animated rows, because the overlay covers the screen.
        let _ = render_full(&state, 160, 48);
        assert_eq!(
            crate::tui::ui::last_idle_animation_area(),
            None,
            "the {name} overlay must not publish animated rows"
        );

        let interval = crate::tui::redraw_interval(&state);
        assert!(
            interval >= crate::tui::REDRAW_IDLE,
            "with the {name} overlay up there is no visible animation, so the \
             loop must not run at animation cadence; got {interval:?}"
        );
        assert!(
            !crate::tui::periodic_redraw_required(&state),
            "with the {name} overlay up a periodic tick must not demand a frame \
             for an animation nobody can see"
        );
    }
}

/// The invariant behind the fix: whenever the loop is paced at animation
/// cadence, the renderer must have published animated rows for the cheap partial
/// repaint to patch. Otherwise every tick is a full frame that changes nothing.
///
/// Checked across terminal sizes and both idle screens, which is how the second
/// instance of this bug was found: on a short terminal the onboarding donut
/// shrinks to zero rows, so nothing animates there either.
#[test]
fn animation_cadence_implies_the_renderer_published_animated_rows() {
    let _lock = viewport_snapshot_test_lock();
    clear_flicker_frame_history_for_tests();

    let screens: Vec<(&str, TestState)> = vec![
        ("idle", idle_animation_state(1.0)),
        (
            "onboarding",
            TestState {
                onboarding_preview: true,
                suggestions: vec![
                    ("Log in to get started".to_string(), "/login".to_string()),
                    ("Build a CLI".to_string(), "build a CLI".to_string()),
                ],
                anim_elapsed: 1.0,
                ..Default::default()
            },
        ),
    ];

    for (name, state) in screens {
        for (width, height) in [(160u16, 48u16), (100, 40), (80, 24), (60, 12), (40, 8)] {
            let _ = render_full(&state, width, height);
            let published = crate::tui::ui::last_idle_animation_area().is_some();
            let paces_animation = crate::tui::redraw_interval(&state) < crate::tui::REDRAW_IDLE;
            assert!(
                !paces_animation || published,
                "{name} at {width}x{height}: the loop is paced at animation \
                 cadence but no animated rows were published, so every tick \
                 becomes a full-frame render that changes nothing"
            );
        }
    }
}

/// The cadence is derived from what the renderer last published, so the obvious
/// hazard is a deadlock: "no donut on screen" must not be able to prevent a
/// donut from ever appearing.
///
/// It cannot, because the renderer decides independently (`ui::draw` consults
/// `idle_donut_active`, never the on-screen variant), and the first frame after
/// any state change is drawn on demand rather than on this cadence. This pins
/// that bootstrap: from "nothing published", one ordinary frame is enough to get
/// the animation on screen and the loop back to animation cadence.
#[test]
fn the_animation_can_still_start_from_a_state_with_nothing_published() {
    let _lock = viewport_snapshot_test_lock();
    clear_flicker_frame_history_for_tests();

    // Simulate "an overlay was up, so nothing is published".
    crate::tui::ui::record_idle_animation_area(None);

    let idle = idle_animation_state(1.0);
    assert!(
        crate::tui::idle_donut_active(&idle),
        "the renderer's own decision must not depend on what was last published, \
         otherwise the animation could never start"
    );

    // One on-demand frame is enough to publish the rows again.
    let _ = render_full(&idle, 160, 48);
    assert!(
        crate::tui::ui::last_idle_animation_area().is_some(),
        "a single frame must be able to bring the animation back on screen"
    );
    assert!(
        crate::tui::periodic_redraw_required(&idle),
        "with the animation on screen again the loop must resume animation ticks"
    );
}
