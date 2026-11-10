/// The regression that made the animation lag: an idle screen almost always
/// has some slow-moving chrome up (notification line, status notice, cache
/// countdown). If that chrome wins every tick, the animation runs at full
/// frame cost. It must instead get a full frame only at its own cadence,
/// with the animation frames in between served cheaply.
/// The animation-only path reuses the previous full frame for everything
/// outside the decorative rows, so it can never show a newer input line. It
/// must therefore refuse to run while the composer differs from what the last
/// full frame drew.
///
/// Without this, a keystroke's redraw request was "satisfied" by an animation
/// frame that did not contain the character, and the glyph waited for a later
/// full frame. Measured against a live client, typing echoed to the terminal
/// in ~7ms but only reached the screen ~500ms later; with the guard the same
/// keystroke paints in ~6ms.
///
/// Every existing test missed this because they render single frames and
/// compare pixels: the partial frame is *correct*, it is simply the wrong
/// frame to have drawn, which is only visible across frames over time.
#[test]
fn the_animation_only_path_refuses_to_run_when_the_composer_changed() {
    // Pretend a full frame drew an empty composer.
    // Struct update so a new renderer field cannot break this test.
    let renderer = StatusSpinnerRenderer {
        last_frame: Some(Buffer::empty(Rect::new(0, 0, 10, 3))),
        ..Default::default()
    };

    // Same input: the guard must not object (other predicates may still
    // block, which is why this asserts the reason rather than the outcome).
    assert!(
        !renderer.composer_changed_since_last_full_frame(""),
        "an unchanged composer must not block the cheap path"
    );

    // A keystroke landed. The path cannot render it, so it must be blocked.
    assert!(
        renderer.composer_changed_since_last_full_frame("/"),
        "a typed character must force a full frame"
    );
}

/// Invalidation must clear the remembered input too. A stale value would let
/// the animation-only path run against a frame that no longer exists.
#[test]
fn invalidating_the_renderer_forgets_the_drawn_composer() {
    let mut renderer = StatusSpinnerRenderer {
        last_full_frame_input: "draft".to_string(),
        ..Default::default()
    };
    renderer.invalidate();
    assert!(
        renderer.composer_changed_since_last_full_frame("draft"),
        "after invalidation nothing is known to be drawn, so any composer \
         contents must force a full frame"
    );
}

#[test]
fn partial_frame_retention_matches_presented_cells_across_buffer_swaps() {
    for area in [Rect::new(3, 5, 1, 1), Rect::new(0, 2, 20, 3)] {
        let bounds = Rect::new(0, 0, 20, 8);
        let mut previous = Buffer::empty(bounds);
        previous.set_string(0, 0, "header unchanged", Style::default());
        previous.set_string(0, 7, "composer unchanged", Style::default());
        let mut renderer = StatusSpinnerRenderer {
            last_frame: Some(previous),
            ..Default::default()
        };
        let backend = ratatui::backend::TestBackend::new(20, 8);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let storage = renderer.last_frame.as_ref().unwrap().content.as_ptr();
        for glyph in ["⠋", "⠙", "⠹", "⠸"] {
            let working = terminal.current_buffer_mut();
            working
                .content
                .clone_from(&renderer.last_frame.as_ref().unwrap().content);
            for y in area.top()..area.bottom() {
                for x in area.left()..area.right() {
                    working[(x, y)].set_symbol(glyph).set_fg(Color::Cyan);
                }
            }
            let presented = working.clone();
            terminal.flush().unwrap();
            renderer.remember_partial_frame(terminal.current_buffer_mut(), area);
            terminal.swap_buffers();
            assert_eq!(renderer.last_frame.as_ref().unwrap(), &presented);
            assert_eq!(
                renderer.last_frame.as_ref().unwrap().content.as_ptr(),
                storage
            );
            assert_eq!(terminal.backend().buffer(), &presented);
        }
    }
}

#[test]
#[ignore = "comparative retained-frame benchmark, run with --ignored --nocapture"]
fn benchmark_partial_frame_retention() {
    use std::hint::black_box;
    let bounds = Rect::new(0, 0, 160, 48);
    let animation_area = Rect::new(0, 3, 160, 14);
    let spinner_area = Rect::new(2, 46, 1, 1);
    let iterations = 200;
    let initial = Buffer::empty(bounds);
    // Exclude one-time animation/config initialization from the comparison.
    let mut warmup = initial.clone();
    crate::tui::ui::render_idle_animation_into(&mut warmup, animation_area, 0.0);
    for animation in [false, true] {
        let mut times = Vec::new();
        let mut baseline_frame = None;
        for optimized in [false, true] {
            let mut renderer = StatusSpinnerRenderer {
                last_frame: Some(initial.clone()),
                ..Default::default()
            };
            let mut working = initial.clone();
            let start = Instant::now();
            for i in 0..iterations {
                let previous = renderer.last_frame.as_ref().unwrap();
                if optimized {
                    working.content.clone_from(&previous.content);
                } else {
                    working.clone_from(previous);
                }
                if animation {
                    let elapsed = i as f32 / 30.0;
                    crate::tui::ui::render_idle_animation_into(
                        &mut working,
                        animation_area,
                        elapsed,
                    );
                    if optimized {
                        renderer.remember_partial_frame(&working, animation_area);
                    } else {
                        crate::tui::ui::render_idle_animation_into(
                            renderer.last_frame.as_mut().unwrap(),
                            animation_area,
                            elapsed,
                        );
                    }
                } else {
                    render_status_spinner_into_buffer_mut(&mut working, spinner_area, "⠙");
                    if optimized {
                        renderer.remember_partial_frame(&working, spinner_area);
                    } else {
                        renderer.last_frame = Some(working.clone());
                    }
                }
                black_box(renderer.last_frame.as_ref().unwrap());
            }
            times.push(start.elapsed());
            if optimized {
                assert_eq!(renderer.last_frame, baseline_frame);
            } else {
                baseline_frame = renderer.last_frame;
            }
        }
        eprintln!(
            "partial retention animation={animation} iterations={iterations} baseline={:?} optimized={:?} speedup={:.2}x",
            times[0],
            times[1],
            times[0].as_secs_f64() / times[1].as_secs_f64()
        );
    }
}

use super::*;
use ratatui::style::Color;

#[tokio::test]
async fn redraw_timer_waits_one_period_and_skips_missed_ticks() {
    let mut timer = redraw_timer(Duration::from_millis(250));
    assert!(
        tokio::time::timeout(Duration::from_millis(20), timer.tick())
            .await
            .is_err(),
        "the first redraw tick must not fire immediately"
    );
    assert_eq!(
        timer.missed_tick_behavior(),
        tokio::time::MissedTickBehavior::Skip
    );
}

fn assert_duration_close(actual: Duration, expected: Duration) {
    let actual_ms = actual.as_millis() as i128;
    let expected_ms = expected.as_millis() as i128;
    assert!(
        (actual_ms - expected_ms).abs() <= 1,
        "expected {actual:?} to be within 1ms of {expected:?}"
    );
}

#[test]
fn status_spinner_fast_path_uses_status_elapsed_clock() {
    let full_status_elapsed = 0.0;
    let app_lifetime_elapsed = 0.24;

    let full_status_symbol =
        jcode_tui_style::theme::activity_indicator(full_status_elapsed, STATUS_SPINNER_FPS, true);
    let old_app_lifetime_symbol =
        jcode_tui_style::theme::activity_indicator(app_lifetime_elapsed, STATUS_SPINNER_FPS, true);
    let fast_path_symbol = jcode_tui_style::theme::activity_indicator(
        status_spinner_elapsed_for_sources(Some(full_status_elapsed)),
        STATUS_SPINNER_FPS,
        true,
    );

    assert_ne!(
        old_app_lifetime_symbol, full_status_symbol,
        "the app lifetime clock can be on a different spinner frame than the status clock"
    );
    assert_eq!(fast_path_symbol, full_status_symbol);
}

#[test]
fn primary_spinner_statuses_are_explicit() {
    assert!(status_uses_primary_spinner(&ProcessingStatus::Sending));
    assert!(status_uses_primary_spinner(&ProcessingStatus::Streaming));
    assert!(status_uses_primary_spinner(&ProcessingStatus::RunningTool(
        "bash".to_string()
    )));
    assert!(!status_uses_primary_spinner(&ProcessingStatus::Idle));
    assert!(!status_uses_primary_spinner(
        &ProcessingStatus::WaitingForNetwork {
            listener: "network".to_string(),
        }
    ));
}

#[test]
fn slash_command_palette_suspends_spinner_fast_path() {
    assert!(is_slash_command_input("/"));
    assert!(is_slash_command_input("  /help"));
    assert!(!is_slash_command_input("normal prompt"));
}

/// The palette predicate must short-circuit on the input prefix so hot
/// paths never build the suggestion list for ordinary prompts.
#[test]
fn palette_visibility_checks_the_cheap_prefix_before_building_suggestions() {
    let mut suggestions_built = false;
    assert!(!slash_command_palette_may_be_visible("hello", || {
        suggestions_built = true;
        true
    }));
    assert!(
        !suggestions_built,
        "non-slash input must not pay for the suggestion list"
    );

    assert!(slash_command_palette_may_be_visible("/", || true));
    assert!(!slash_command_palette_may_be_visible("/", || false));
}

/// The bug this pins: on a fresh idle screen (donut spinning), pressing `/`
/// opens the command palette, which floats over the animation rows. Full
/// frames painted the palette, then the very next animation-only repaint
/// reset those rows and erased it, so the menu blinked in and out at the
/// chrome full-frame cadence (~4 Hz), captured frame-by-frame from a live
/// tester PTY. While the palette may be visible, animation ticks must take
/// the full-frame path, which re-renders the overlay.
#[test]
fn the_animation_only_path_refuses_to_run_while_the_command_palette_is_up() {
    let clean_idle_frame = IdleAnimationFastPathInputs {
        has_previous_frame: true,
        animation_active: true,
        has_animation_area: true,
        force_full_redraw: false,
        force_full_repaint: false,
        composer_changed: false,
        command_palette_visible: false,
    };
    assert_eq!(
        idle_animation_fast_path_blocked_reason(&clean_idle_frame),
        None,
        "a clean idle frame must keep the cheap path available"
    );

    let palette_open = IdleAnimationFastPathInputs {
        command_palette_visible: true,
        ..clean_idle_frame
    };
    assert_eq!(
        idle_animation_fast_path_blocked_reason(&palette_open),
        Some("command_palette_visible"),
        "an open palette must force full frames so the overlay survives"
    );

    // A keystroke that both changes the composer and opens the palette
    // reports the keystroke: it is the more urgent of the two reasons.
    let typing_into_palette = IdleAnimationFastPathInputs {
        composer_changed: true,
        command_palette_visible: true,
        ..clean_idle_frame
    };
    assert_eq!(
        idle_animation_fast_path_blocked_reason(&typing_into_palette),
        Some("input_changed"),
    );
}

#[test]
fn status_spinner_reset_targets_next_frame_boundary() {
    assert_duration_close(
        status_spinner_delay_until_next_frame(0.0),
        STATUS_SPINNER_ONLY_INTERVAL,
    );
    assert_duration_close(
        status_spinner_delay_until_next_frame(0.040),
        Duration::from_millis(40),
    );
    assert_duration_close(
        status_spinner_delay_until_next_frame(1.0),
        Duration::from_millis(40),
    );
    assert_duration_close(
        status_spinner_delay_until_next_frame(f32::NAN),
        STATUS_SPINNER_ONLY_INTERVAL,
    );
}

#[test]
fn status_spinner_partial_mutates_only_status_cell() {
    let area = Rect::new(0, 0, 8, 2);
    let mut buffer = Buffer::empty(area);
    buffer.set_string(0, 0, "abcdefgh", Style::default().fg(Color::White));
    buffer.set_string(0, 1, "ABCDEFGH", Style::default().fg(Color::Blue));
    buffer
        .cell_mut((2, 1))
        .expect("status cell")
        .set_symbol("⠋");
    let before = buffer.clone();

    let status_area = Rect::new(2, 1, 6, 1);
    assert!(render_status_spinner_into_buffer(&buffer, status_area, "⠙"));
    render_status_spinner_into_buffer_mut(&mut buffer, status_area, "⠙");

    for y in 0..2 {
        for x in 0..8 {
            if (x, y) == (2, 1) {
                assert_eq!(buffer.cell((x, y)).unwrap().symbol(), "⠙");
                assert_eq!(
                    buffer.cell((x, y)).unwrap().fg,
                    jcode_tui_style::theme::ai_color()
                );
            } else {
                assert_eq!(buffer.cell((x, y)), before.cell((x, y)));
            }
        }
    }
}

#[test]
fn status_spinner_partial_does_not_overwrite_slash_palette_cell() {
    let area = Rect::new(0, 0, 12, 1);
    let mut buffer = Buffer::empty(area);
    buffer.set_string(0, 0, "/help  show help", Style::default().fg(Color::Yellow));

    assert!(
        !render_status_spinner_into_buffer(&buffer, area, "⠙"),
        "late overlays own the status cell until the next full frame"
    );
}

#[test]
fn presented_animation_frame_requires_reseeding() {
    let animation_area = Rect::new(0, 1, 10, 2);
    let mut renderer = StatusSpinnerRenderer {
        seeded_animation_area: Some(animation_area),
        ..Default::default()
    };
    let backend = ratatui::backend::TestBackend::new(10, 3);
    let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");

    renderer.swap_after_animation_frame(&mut terminal);

    assert_eq!(renderer.seeded_animation_area, None);
}

#[test]
fn idle_animation_repaint_preserves_cells_outside_animation_after_swap() {
    let area = Rect::new(0, 0, 20, 4);
    let animation_area = Rect::new(0, 3, 20, 1);
    let mut previous_frame = Buffer::empty(area);
    previous_frame.set_string(0, 0, "transcript", Style::default());
    previous_frame.set_string(0, 2, "> composer", Style::default());
    let mut renderer = StatusSpinnerRenderer::default();
    let backend = ratatui::backend::TestBackend::new(area.width, area.height);
    let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");

    for _ in 0..3 {
        let working = terminal.current_buffer_mut();
        if renderer.seeded_animation_area == Some(animation_area) {
            copy_cells_in(&previous_frame, working, animation_area);
        } else {
            working.clone_from(&previous_frame);
            renderer.seeded_animation_area = Some(animation_area);
        }

        assert_eq!(working[(0, 0)].symbol(), "t");
        assert_eq!(working[(0, 2)].symbol(), ">");

        renderer.swap_after_animation_frame(&mut terminal);
    }
}
