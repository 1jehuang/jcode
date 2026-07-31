// Redraw-cadence policy: which app states justify repainting at animation
// speed, and which must not.
//
// This exists because of a real, user-visible bug: a transient status notice
// (e.g. "Swarm plan synced ...") put the whole client on the animation cadence.
// At 60fps that is ~180 full frames per notice, each re-deriving the transcript,
// header, status line, and composer into essentially identical cells. Keystrokes
// landed behind one of those frames, so a freshly spawned session felt laggy,
// and a notice that kept re-arming held the client there indefinitely.
//
// `scripts/repro_input_lag.py --live` measures the same thing end to end against
// a real binary; these tests are the cheap gate that runs in CI.

/// A state whose only "live" element is a piece of static text chrome.
fn static_chrome_state(notice: Option<&str>) -> TestState {
    TestState {
        // A started conversation, so the decorative idle donut (which legitimately
        // wants animation cadence) is not what we end up measuring.
        display_messages: vec![DisplayMessage {
            role: "user".to_string(),
            content: "a real prompt".to_string(),
            tool_calls: vec![],
            duration_secs: None,
            title: None,
            tool_data: None,
        }],
        status_notice: notice.map(str::to_string),
        status: ProcessingStatus::Idle,
        time_since_activity: Some(Duration::from_secs(1)),
        ..Default::default()
    }
}

fn full_tier_policy() -> crate::perf::TuiPerfPolicy {
    // Build from the real policy so a new field cannot silently drift, then pin
    // the parts these assertions depend on. The host's load average must not
    // decide whether this test passes.
    crate::perf::TuiPerfPolicy {
        tier: crate::perf::PerformanceTier::Full,
        enable_decorative_animations: true,
        animation_fps: 60,
        redraw_fps: 60,
        ..crate::perf::tui_policy()
    }
}

/// The regression gate: a status notice must not pull the redraw loop to
/// animation cadence.
#[test]
fn a_status_notice_does_not_force_animation_cadence() {
    let policy = full_tier_policy();
    let animation_interval = Duration::from_millis(1000 / u64::from(policy.animation_fps.max(1)));

    let quiet = crate::tui::redraw_interval_with_policy(&static_chrome_state(None), &policy);
    let with_notice = crate::tui::redraw_interval_with_policy(
        &static_chrome_state(Some("Swarm plan synced (v55, 98 items)")),
        &policy,
    );

    assert!(
        with_notice > animation_interval,
        "a static notice must not repaint at animation cadence \
         (got {with_notice:?}, animation is {animation_interval:?})"
    );
    assert!(
        with_notice <= Duration::from_millis(250),
        "the notice still has to retire promptly (got {with_notice:?})"
    );
    // A notice may legitimately tick a little faster than deep idle, but it must
    // not be dramatically more expensive than the same screen without it.
    assert!(
        with_notice >= quiet / 4,
        "a notice must not cost many times the quiet cadence \
         (notice {with_notice:?} vs quiet {quiet:?})"
    );
}

/// The fix must not slow down states that genuinely animate: streaming output
/// still needs the fast cadence even though a notice may be on screen too.
#[test]
fn streaming_output_keeps_the_fast_cadence_even_with_a_notice() {
    let policy = full_tier_policy();
    let fast_interval = Duration::from_millis(1000 / u64::from(policy.redraw_fps.max(1)));

    let mut state = static_chrome_state(Some("Swarm plan synced"));
    state.streaming_text = "partial assistant answer".to_string();

    assert_eq!(
        crate::tui::redraw_interval_with_policy(&state, &policy),
        fast_interval,
        "streaming must still repaint at the fast cadence"
    );
}

/// `periodic_redraw_required` decides whether a tick draws at all. A notice must
/// still get frames (otherwise it would never appear or retire); the fix is
/// about cadence, not about dropping the notice.
#[test]
fn a_status_notice_still_requires_periodic_frames() {
    assert!(
        crate::tui::periodic_redraw_required(&static_chrome_state(Some("Swarm plan synced"))),
        "a visible notice must still be repainted so it can appear and expire"
    );
}

/// A fresh empty session shows the decorative donut and legitimately animates,
/// not at the idle cadence; there is no input to be responsive about yet.
///
/// The exact rate is the decoration's own capped cadence rather than the full
/// configured `animation_fps`: measured on a real session, 60fps cost 0.224 CPU
/// cores versus 0.104 at 30fps for motion a terminal cannot render any smoother.
/// The cap itself is pinned by
/// `the_decorative_animation_is_capped_below_the_configured_animation_rate`;
/// here we only require that the donut still animates.
#[test]
fn an_idle_empty_session_still_animates_smoothly() {
    let _idle_animation = IdleAnimationEnvGuard::enable();
    // Pin the tier: the auto-detected one depends on host load, and a
    // Reduced/Minimal host legitimately disables the decorative animation.
    crate::perf::pin_full_profile_for_tests();
    let policy = full_tier_policy();
    let idle = TestState {
        time_since_activity: Some(Duration::from_millis(200)),
        ..Default::default()
    };
    let interval = crate::tui::redraw_interval_with_policy(&idle, &policy);
    assert!(
        interval < crate::tui::REDRAW_IDLE,
        "an empty idle screen should animate faster than the idle cadence (got {interval:?})"
    );
    assert!(
        interval <= Duration::from_millis(40),
        "the donut must still read as motion (got {interval:?})"
    );
}

/// While the user is actively typing, the decoration must step aside so
/// keystrokes are not queued behind a 60fps animation frame.
#[test]
fn active_typing_backs_the_decorative_animation_off() {
    let policy = full_tier_policy();
    let fast = Duration::from_millis(1000 / u64::from(policy.animation_fps.max(1)));
    let typing = TestState {
        input: "hel".to_string(),
        cursor_pos: 3,
        time_since_user_interaction: Some(Duration::from_millis(30)),
        ..Default::default()
    };
    let interval = crate::tui::redraw_interval_with_policy(&typing, &policy);
    assert!(
        interval > fast,
        "typing must not compete with animation frames (got {interval:?})"
    );
}

/// A draft left sitting in the composer must not permanently downgrade the
/// animation: the backoff is about active typing, not about having text.
#[test]
fn a_paused_draft_lets_the_animation_recover() {
    let _idle_animation = IdleAnimationEnvGuard::enable();
    // Pin the tier: the auto-detected one depends on host load, and a
    // Reduced/Minimal host legitimately disables the decorative animation.
    crate::perf::pin_full_profile_for_tests();
    let policy = full_tier_policy();
    let paused = TestState {
        input: "a draft i walked away from".to_string(),
        cursor_pos: 5,
        time_since_user_interaction: Some(Duration::from_secs(3)),
        ..Default::default()
    };
    let interval = crate::tui::redraw_interval_with_policy(&paused, &policy);
    assert!(
        interval < crate::tui::REDRAW_IDLE && interval <= Duration::from_millis(40),
        "the animation must recover once typing stops (got {interval:?})"
    );
}

/// The decorative idle animation must be capped below the configured animation
/// rate, because its cost is linear in frame rate while its perceived smoothness
/// is not.
///
/// Measured on a real session with `scripts/sweep_animation_fps.py`: 60fps costs
/// 0.224 CPU cores over an idle baseline, 30fps costs 0.104, and keystroke
/// latency is identical at both. Spending a fifth of a core to animate a
/// decoration is the difference the user felt as "spawning a new one still lags".
#[test]
fn the_decorative_animation_is_capped_below_the_configured_animation_rate() {
    let _idle_animation = IdleAnimationEnvGuard::enable();
    // Pin the tier: the auto-detected one depends on host load, and a
    // Reduced/Minimal host legitimately disables the decorative animation.
    crate::perf::pin_full_profile_for_tests();
    let policy = full_tier_policy();
    assert_eq!(
        policy.animation_fps, 60,
        "this test assumes the 60fps default it is protecting against"
    );
    let configured = Duration::from_millis(1000 / u64::from(policy.animation_fps));

    let idle = TestState {
        time_since_activity: Some(Duration::from_millis(200)),
        ..Default::default()
    };
    let interval = crate::tui::redraw_interval_with_policy(&idle, &policy);
    assert!(
        interval > configured,
        "the decoration must not run at the full configured rate (got {interval:?})"
    );
    // Still smooth enough to read as motion rather than stepping.
    assert!(
        interval <= Duration::from_millis(40),
        "the decoration must still look like motion (got {interval:?})"
    );
}

/// A user who configures a *lower* animation rate must keep it: the cap is a
/// ceiling, not an override.
#[test]
fn a_lower_configured_animation_rate_is_respected() {
    let _idle_animation = IdleAnimationEnvGuard::enable();
    // Pin the tier: the auto-detected one depends on host load, and a
    // Reduced/Minimal host legitimately disables the decorative animation.
    crate::perf::pin_full_profile_for_tests();
    let mut policy = full_tier_policy();
    policy.animation_fps = 10;
    let idle = TestState {
        time_since_activity: Some(Duration::from_millis(200)),
        ..Default::default()
    };
    assert_eq!(
        crate::tui::redraw_interval_with_policy(&idle, &policy),
        Duration::from_millis(100),
        "a configured 10fps must not be raised by the decorative cap"
    );
}
