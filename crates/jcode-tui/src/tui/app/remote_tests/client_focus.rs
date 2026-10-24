#[test]
fn client_focus_defaults_to_true() {
    let app = create_test_app();
    assert!(
        app.client_focused(),
        "a freshly created client should start focused so terminals that never \
         report focus events still animate/redraw normally"
    );
}

#[test]
fn idle_donut_pauses_while_unfocused() {
    let mut app = create_test_app();

    // Whether the donut runs while focused depends on the machine's perf tier
    // and `display.idle_animation` config, so we do not assert the focused case
    // absolutely. We only assert the invariant that matters for the swarm CPU
    // regression: it must never run while the terminal is unfocused.
    let redraw = app.set_client_focused(false);
    assert!(
        !redraw,
        "losing focus should not request an immediate redraw"
    );
    assert!(!app.client_focused());
    assert!(
        !crate::tui::idle_donut_active(&app),
        "idle animation must pause while the terminal is unfocused"
    );

    // Regaining focus requests a differential redraw so the window catches up
    // without clearing and retransmitting every terminal cell.
    app.force_full_redraw = false;
    let redraw = app.set_client_focused(true);
    assert!(redraw, "regaining focus should request a redraw");
    assert!(app.client_focused());
    assert!(
        !app.force_full_redraw,
        "focus changes must not force an expensive full-terminal repaint"
    );
}

#[test]
fn unfocused_redraw_warranted_tracks_live_activity() {
    let mut app = create_test_app();
    // `unfocused_redraw_warranted` is only consulted while unfocused, and the
    // decorative donut is force-disabled when unfocused, so evaluate it in that
    // state to mirror the run loop.
    app.set_client_focused(false);

    // Idle empty session: no live output to paint while unfocused.
    assert!(
        !app.unfocused_redraw_warranted(),
        "an idle unfocused session has nothing changing worth a full-rate redraw"
    );

    // A streaming/processing session keeps painting even while unfocused so a
    // visible-but-unfocused window in a tiling WM still shows live progress.
    app.is_processing = true;
    assert!(
        app.unfocused_redraw_warranted(),
        "a processing session should keep redrawing while unfocused"
    );
}

#[test]
fn client_interaction_restores_focus_so_scroll_redraws_at_full_rate() {
    // Regression for the intermittent "can't scroll" bug. If a FocusGained is
    // dropped (flaky under tiling WMs / multiplexers) the window can get stuck
    // as "unfocused idle", which the run loop throttles to ~1 Hz. Any terminal
    // input (key/mouse/scroll) is only delivered to the focused window, so it
    // must restore the focused state and full-rate redraws immediately.
    let mut app = create_test_app();

    // Simulate a stuck-unfocused window (FocusLost seen, FocusGained dropped).
    app.set_client_focused(false);
    assert!(!app.client_focused());
    assert!(
        !app.unfocused_redraw_warranted(),
        "an idle unfocused session is throttled to ~1 Hz redraws"
    );

    // A mouse-wheel / key event arrives: the terminal only routes input to the
    // focused window, so interacting proves focus and must restore it.
    app.note_client_interaction();
    assert!(
        app.client_focused(),
        "interaction must restore focus so scrolling repaints at full rate"
    );
}
