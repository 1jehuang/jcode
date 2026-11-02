//! Overlay wakeups must reflect unfinished work, not the underlying session's
//! activity clock or the decorative animation policy.
use super::*;
use crate::tui::redraw_schedule::{
    REDRAW_DEEP_IDLE, REDRAW_IDLE, current_full_frame_redraw_reason, periodic_redraw_required,
    periodic_redraw_required_excluding_idle_animation, redraw_interval_with_policy_and_animation,
};

fn policy(tier: crate::perf::PerformanceTier) -> crate::perf::TuiPerfPolicy {
    crate::perf::TuiPerfPolicy {
        tier,
        enable_decorative_animations: false,
        animation_fps: 1,
        redraw_fps: 1,
        ..crate::perf::tui_policy()
    }
}

fn state_with_picker(deep_idle: bool) -> TestState {
    TestState {
        session_picker: Some(std::rc::Rc::new(std::cell::RefCell::new(
            session_picker::SessionPicker::new(Vec::new()),
        ))),
        time_since_activity: Some(Duration::from_secs(if deep_idle { 300 } else { 1 })),
        time_since_user_interaction: Some(Duration::from_secs(if deep_idle { 300 } else { 1 })),
        ..Default::default()
    }
}

#[test]
fn focused_pending_preview_work_overrides_deep_idle_and_animation_policy() {
    let _guard = viewport_snapshot_test_lock();
    for workspace in [false, true] {
        for deep_idle in [false, true] {
            let state = state_with_picker(deep_idle);
            state
                .session_picker
                .as_ref()
                .unwrap()
                .borrow_mut()
                .set_pending_preview_for_test(workspace);
            for tier in [
                crate::perf::PerformanceTier::Full,
                crate::perf::PerformanceTier::Minimal,
            ] {
                assert_eq!(
                    redraw_interval_with_policy_and_animation(&state, &policy(tier), false),
                    Duration::from_millis(16),
                );
            }
            assert!(periodic_redraw_required(&state));
            assert!(periodic_redraw_required_excluding_idle_animation(&state));
            assert_eq!(
                current_full_frame_redraw_reason(&state),
                Some("session_picker_preview")
            );
        }
    }
}

#[test]
fn settled_or_closed_picker_has_no_permanent_repaint_and_restores_idle_cadence() {
    let _guard = viewport_snapshot_test_lock();
    for deep_idle in [false, true] {
        let mut state = state_with_picker(deep_idle);
        for overlay_open in [true, false] {
            if !overlay_open {
                state.session_picker = None;
            }
            assert_eq!(
                redraw_interval_with_policy_and_animation(
                    &state,
                    &policy(crate::perf::PerformanceTier::Full),
                    false,
                ),
                if deep_idle {
                    REDRAW_DEEP_IDLE
                } else {
                    REDRAW_IDLE
                },
            );
            assert!(!periodic_redraw_required_excluding_idle_animation(&state));
            assert_eq!(current_full_frame_redraw_reason(&state), None);
        }
    }
}

#[test]
fn rendering_settled_receivers_stops_the_fast_poll_loop() {
    let _guard = viewport_snapshot_test_lock();
    for workspace in [false, true] {
        let state = state_with_picker(true);
        let picker = state.session_picker.as_ref().unwrap();
        picker.borrow_mut().set_pending_preview_for_test(workspace);
        assert!(periodic_redraw_required_excluding_idle_animation(&state));
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 40)).unwrap();
        terminal.draw(|frame| picker.borrow_mut().render(frame)).unwrap();
        assert!(!picker.borrow().has_pending_preview_work());
        assert!(!periodic_redraw_required_excluding_idle_animation(&state));
        assert_eq!(
            redraw_interval_with_policy_and_animation(
                &state, &policy(crate::perf::PerformanceTier::Full), false,
            ),
            REDRAW_DEEP_IDLE,
        );
    }
}

#[test]
fn background_pending_preview_does_not_burn_frames_and_focus_resumes_polling() {
    let _guard = viewport_snapshot_test_lock();
    for workspace in [false, true] {
        let mut state = state_with_picker(true);
        state
            .session_picker
            .as_ref()
            .unwrap()
            .borrow_mut()
            .set_pending_preview_for_test(workspace);
        state.unfocused = true;
        assert_eq!(
            redraw_interval_with_policy_and_animation(
                &state,
                &policy(crate::perf::PerformanceTier::Full),
                false,
            ),
            REDRAW_DEEP_IDLE,
        );
        assert!(!periodic_redraw_required(&state));
        assert!(!periodic_redraw_required_excluding_idle_animation(&state));
        assert_eq!(current_full_frame_redraw_reason(&state), None);
        state.unfocused = false;
        assert_eq!(
            redraw_interval_with_policy_and_animation(
                &state,
                &policy(crate::perf::PerformanceTier::Full),
                false,
            ),
            Duration::from_millis(16),
        );
        assert!(periodic_redraw_required_excluding_idle_animation(&state));
    }
}
