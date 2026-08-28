//! Redraw scheduling: how often the TUI repaints, and whether a periodic tick
//! needs a frame at all.
//!
//! Split out of `tui/mod.rs` so the policy that decides "repaint now, and how
//! fast" lives in one readable place. Two questions are answered here:
//!
//! - [`redraw_interval`]: the tick cadence for the current app state, from a
//!   5s deep-idle crawl up to the configured animation FPS.
//! - [`periodic_redraw_required`]: whether a tick should actually draw. The
//!   `_excluding_idle_animation` variant lets the run loop separate "real state
//!   changed" from "only the decorative animation advanced", so the latter is
//!   served by an animation-only partial repaint instead of a full frame.

use super::*;

pub(crate) const REDRAW_IDLE: Duration = Duration::from_millis(250);
pub(crate) const REDRAW_DEEP_IDLE: Duration = Duration::from_millis(5000);
pub(crate) const REDRAW_REMOTE_STARTUP: Duration = Duration::from_millis(1000);
pub(crate) const REDRAW_PASSIVE_LIVENESS: Duration = Duration::from_millis(1000);
pub(crate) const REDRAW_DEEP_IDLE_AFTER: Duration = Duration::from_secs(30);

/// Whether this session has been left alone long enough to be treated as
/// dormant (deep idle): no stream activity *and* no user interaction for
/// [`REDRAW_DEEP_IDLE_AFTER`].
///
/// `time_since_activity()` alone is not a dormancy signal. It reports
/// "already past the deep-idle threshold" for any non-empty transcript that
/// has never streamed in this process (see `TuiState::time_since_activity`),
/// which is correct for a restored historical session but also matches a
/// brand-new session the moment onboarding leaves its "here are a few things
/// to try" notice. The user is sitting right there, actively pressing keys,
/// while every deep-idle consumer (donut gate, tick cadence, periodic-redraw
/// short-circuit) treats the session as abandoned. That is how the decorative
/// animation ended up never running on the screen it was built for.
///
/// A recent keystroke/mouse/paste is direct evidence the session is not
/// dormant, so it must hold deep idle off for the same window.
fn deep_idle_dormant(state: &dyn TuiState) -> bool {
    let stream_dormant = state
        .time_since_activity()
        .map(|d| d >= REDRAW_DEEP_IDLE_AFTER)
        .unwrap_or(false);
    let user_dormant = state
        .time_since_user_interaction()
        .map(|d| d >= REDRAW_DEEP_IDLE_AFTER)
        // No interaction recorded yet: a fresh client that has never been
        // touched. Fall back to the stream/app clock alone, as before.
        .unwrap_or(true);
    stream_dormant && user_dormant
}

fn idle_donut_active_with_policy(
    state: &dyn TuiState,
    policy: &crate::perf::TuiPerfPolicy,
) -> bool {
    if state.remote_startup_phase_active() {
        return false;
    }

    // Decorative animations are purely visual; never spin them while the terminal
    // window/tab is backgrounded. A swarm of unfocused sessions would otherwise
    // each render a full-screen 3D scene at animation FPS, saturating every core.
    if !state.client_focused() {
        return false;
    }

    // The onboarding welcome screen is static (no decorative animation), so it
    // does not need to keep the animation loop running.
    if state.onboarding_welcome_active() {
        return false;
    }

    // The idle donut is decorative.  Leaving many dormant tabs/sessions open
    // should not keep every TUI repainting forever, especially when those tabs
    // are hidden behind a terminal multiplexer or kitty single-instance window.
    if deep_idle_dormant(state) {
        return false;
    }

    policy.enable_decorative_animations
        && crate::config::config().display.idle_animation
        && policy.tier.idle_animation_enabled()
        && !has_started_conversation(state)
        && !state.is_processing()
        && state.streaming_text().is_empty()
        && state.queued_messages().is_empty()
}

/// Whether the transcript contains any real conversation yet (a user prompt or
/// an assistant/tool/reasoning reply). A fresh screen that only holds
/// non-conversational notices (e.g. the "run /login when you're ready" system
/// message left after onboarding is declined) is still "idle", so the decorative
/// donut should keep spinning until the user actually starts chatting.
fn has_started_conversation(state: &dyn TuiState) -> bool {
    state
        .display_messages()
        .iter()
        .any(|m| matches!(m.role.as_str(), "user" | "assistant" | "tool" | "reasoning"))
}

/// Whether the decorative animation is actually *on screen*, which is what the
/// redraw cadence must be paced by.
///
/// [`idle_donut_active`] answers a different question: "does this screen want a
/// donut?". The renderer uses that to lay out donut rows, and the two answers
/// can differ, because layout can drop the animation after the fact:
///
/// * `ui::draw_inner` returns early for full-screen overlays (`/resume` picker,
///   help, changelog, model status, login/account pickers) long before the donut
///   chunk is laid out.
/// * The donut reservation shrinks as the composer grows, and on a short
///   terminal it reaches zero rows.
///
/// In both cases nothing animates, yet the loop still ticked at `animation_fps`.
/// Because no animated rectangle was published, the cheap animation-only repaint
/// stood down (`no_animation_area`) and *every* tick became a full frame.
/// Measured on a live client freshly spawned onto the `/resume` picker: 63 full
/// `terminal.draw` calls per second, each changing 0 of 7680 cells, on a screen
/// a terminal emulator confirmed was completely static. Keystrokes competed with
/// 60 useless full frames a second, which is the reported lag.
///
/// `animation_on_screen` is passed in rather than read from the renderer's
/// global slot so this stays a pure function of stated inputs: the callers that
/// pace the live loop supply [`ui::last_idle_animation_area`], while tests can
/// state the premise directly. Deriving it from what the renderer actually
/// published (rather than re-deriving layout rules here) means a new overlay or
/// layout tweak cannot reintroduce the wasted loop.
///
/// The renderer must never consult this when deciding whether to draw the donut:
/// that would be self-referential, and "nothing published last frame" would
/// prevent a donut forever.
fn idle_donut_paces_redraws(
    state: &dyn TuiState,
    policy: &crate::perf::TuiPerfPolicy,
    animation_on_screen: bool,
) -> bool {
    animation_on_screen && idle_donut_active_with_policy(state, policy)
}

/// Whether the renderer published animated rows on its last frame.
///
/// The live default for `animation_on_screen`. A brand-new client has published
/// nothing yet, and its first frame is drawn on demand rather than on the
/// animation cadence, so starting from `false` costs at most one tick of
/// smoothness and never blocks the animation from starting.
fn animation_on_screen_now() -> bool {
    ui::last_idle_animation_area().is_some()
}

/// Last reason a periodic tick demanded a full frame instead of the cheap
/// animation-only repaint, surfaced through `draw-stats`.
///
/// Stored as an index into [`FULL_FRAME_REDRAW_REASONS`] in an atomic, so the
/// redraw hot path records it without locking (and without an error to ignore).
static LAST_FULL_FRAME_REDRAW_REASON: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(usize::MAX);

const FULL_FRAME_REDRAW_REASONS: &[&str] = &[
    "processing",
    "streaming",
    "tail_catchup",
    "status_notice",
    "learn_hint",
    "mouse_scroll_animation",
    "copy_autoscroll",
    "chat_overscroll",
    "notification",
    "rate_limit_countdown",
    "remote_startup",
    "status_animation",
    "swarm_spinner",
    "session_picker_spinner",
];

fn record_full_frame_redraw_reason(reason: &'static str) {
    if let Some(idx) = FULL_FRAME_REDRAW_REASONS.iter().position(|r| *r == reason) {
        LAST_FULL_FRAME_REDRAW_REASON.store(idx, std::sync::atomic::Ordering::Relaxed);
    }
}

pub(crate) fn last_full_frame_redraw_reason() -> Option<&'static str> {
    FULL_FRAME_REDRAW_REASONS
        .get(LAST_FULL_FRAME_REDRAW_REASON.load(std::sync::atomic::Ordering::Relaxed))
        .copied()
}

/// The reason a full frame is required *right now*, or `None` when nothing is
/// live.
///
/// [`last_full_frame_redraw_reason`] is sticky: it keeps reporting a notice that
/// has long since expired, which makes "why is this client repainting at 60fps"
/// impossible to diagnose from `draw-stats`. This evaluates the predicates
/// against current state instead.
pub(crate) fn current_full_frame_redraw_reason(state: &dyn TuiState) -> Option<&'static str> {
    let policy = crate::perf::tui_policy();
    if full_frame_status_animation_active_with_policy(state, &policy) {
        return Some("status_animation");
    }
    if swarm_spinner_redraw_active(state) {
        return Some("swarm_spinner");
    }
    if session_picker_spinner_redraw_active(state) {
        return Some("session_picker_spinner");
    }
    live_activity_redraw_reason(state)
}

pub(crate) fn idle_donut_active(state: &dyn TuiState) -> bool {
    let policy = crate::perf::tui_policy();
    idle_donut_active_with_policy(state, &policy)
}

fn rate_limit_countdown_redraw_active(state: &dyn TuiState) -> bool {
    state
        .rate_limit_remaining()
        .map(|remaining| remaining <= Duration::from_secs(60))
        .unwrap_or(false)
}

/// The notification line shows a live prompt-cache indicator (`⏳ cache Ns`
/// while warm in the final minute, `🧊 cache cold` once expired). Both states
/// emerge long after the 30s deep-idle cutoff, so without a dedicated wakeup
/// the idle loop never repaints to reveal them. Keep redrawing whenever the
/// cache is within the last-minute countdown window or has just gone cold so
/// the warning actually appears before the next prompt.
fn cache_cold_countdown_redraw_active(state: &dyn TuiState) -> bool {
    if state.is_processing() {
        return false;
    }
    state
        .cache_ttl_status()
        .map(|info| info.is_cold || info.expiring_soon())
        .unwrap_or(false)
}

fn full_frame_status_animation_active_with_policy(
    state: &dyn TuiState,
    policy: &crate::perf::TuiPerfPolicy,
) -> bool {
    if !policy.enable_decorative_animations {
        return false;
    }

    // These animations are rendered as part of the full status line, not by the
    // spinner-only cell renderer in app/run_shell.rs, so they need the normal
    // active redraw loop while visible.
    rate_limit_countdown_redraw_active(state) || crate::build::read_build_progress().is_some()
}

fn primary_status_spinner_fast_path_available_with_policy(
    state: &dyn TuiState,
    _policy: &crate::perf::TuiPerfPolicy,
) -> bool {
    // The single-cell spinner fast path is available in every performance tier,
    // including Minimal/SSH/WSL where decorative animations are off. Keep these
    // conditions in sync with `app::run_shell::status_spinner_only_symbol`, which
    // is what actually gates the spinner-only tick in the run loop.
    state.is_processing()
        && app::run_shell::status_uses_primary_spinner(&state.status())
        && state.streaming_text().is_empty()
        && !state.centered_mode()
        && !state.has_pending_mouse_scroll_animation()
        && !state.remote_startup_phase_active()
}

fn primary_status_spinner_needs_full_redraw_with_policy(
    state: &dyn TuiState,
    policy: &crate::perf::TuiPerfPolicy,
) -> bool {
    // The primary spinner only needs the more expensive full-redraw cadence when
    // the cheap single-cell fast path cannot run (e.g. centered composer). When
    // the fast path is available we keep full redraws at the slow passive-liveness
    // rate and let the one-cell renderer animate the spinner.
    state.is_processing()
        && app::run_shell::status_uses_primary_spinner(&state.status())
        && state.streaming_text().is_empty()
        && !primary_status_spinner_fast_path_available_with_policy(state, policy)
}

/// Redraw cadence while an inline swarm or session-picker spinner is active.
/// This matches the glyph's wall-clock cadence and the primary status spinner:
/// faster wastes unchanged frames, while slower makes the motion visibly step.
pub(crate) const REDRAW_SWARM_SPINNER: Duration =
    Duration::from_millis(jcode_tui_render::swarm_gallery::STRIP_SPINNER_FRAME_MS);

/// Whether the swarm strip (above the status line) or the SwarmStatus dock
/// widget is currently animating a status spinner for an active agent.
///
/// Both surfaces derive the spinner glyph from the wall clock, but managed
/// agents keep running long after the coordinator session itself goes quiet.
/// Without a dedicated wakeup the idle loop stops repainting (deep idle stops
/// it entirely) and the spinner freezes, only twitching when a bus update
/// happens to arrive. Unfocused clients skip this so backgrounded windows do
/// not burn CPU animating a glyph nobody can see; terminal statuses render
/// fixed glyphs and need no animation frames.
fn swarm_spinner_redraw_active(state: &dyn TuiState) -> bool {
    state.client_focused()
        && state
            .inline_swarm_members()
            .iter()
            .any(|m| jcode_tui_render::swarm_gallery::is_active_status(&m.status))
}

/// Whether the open `/resume` picker is showing at least one running session.
/// The picker uses the same 8 fps spinner cells as the swarm strip, so it needs
/// an explicit wakeup even when the session underneath the overlay is idle.
fn session_picker_spinner_redraw_active(state: &dyn TuiState) -> bool {
    state.client_focused()
        && state.session_picker_overlay().is_some_and(|picker| {
            picker
                .try_borrow()
                .ok()
                .is_some_and(|picker| picker.has_visible_running_sessions())
        })
}

fn fps_to_duration(fps: u32) -> Duration {
    Duration::from_millis((1000 / fps.max(1)) as u64)
}

/// Frame rate cap for the purely decorative idle animation.
///
/// Measured on a real session (`scripts/sweep_animation_fps.py`), the cost is
/// linear in frame rate while the perceived motion is not:
///
/// | fps | CPU over idle baseline |
/// |-----|------------------------|
/// | 60  | 0.224 cores            |
/// | 30  | 0.104 cores            |
/// | 20  | 0.080 cores            |
///
/// Each animation frame is a coarse glyph change in a 3x3 subpixel grid, so 30fps
/// already saturates what a terminal can express, and halving the frame rate
/// halves the cost of a decoration that exists to look pleasant while idle.
/// Burning a fifth of a core on it is not a good trade on a laptop.
///
/// This caps only the decorative animation. Functional motion (status spinners,
/// scroll/tail-follow catch-up, streaming output) keeps the configured
/// `animation_fps`, because there smoothness is the feature. Users who
/// explicitly configure a *lower* `animation_fps` still get their value.
const DECORATIVE_ANIMATION_FPS_CAP: u32 = 30;

/// Cadence for the decorative idle animation: the configured animation rate,
/// capped by [`DECORATIVE_ANIMATION_FPS_CAP`].
fn decorative_animation_interval(policy: &crate::perf::TuiPerfPolicy) -> Duration {
    fps_to_duration(policy.animation_fps.min(DECORATIVE_ANIMATION_FPS_CAP))
}

/// Chrome that is text-only and changes on a human timescale: the status notice,
/// the learn hint, and the notification line.
///
/// None of these animate. They appear on an event (which already forces an
/// immediate repaint) and disappear on a multi-second timer, so the loop only
/// needs a tick fast enough to retire them promptly. Treating them as "live"
/// used to pull the whole client to the animation cadence: a single 3s notice
/// meant ~180 full frames of ~10ms each, all to redraw the same glyphs, and
/// every keystroke in that window queued behind one of those frames. A notice
/// that keeps re-arming (a syncing swarm plan, for instance) held a client there
/// indefinitely, which is what made a freshly spawned session feel laggy.
fn static_text_chrome_active(state: &dyn TuiState) -> bool {
    state.status_notice().is_some() || state.learn_hint().is_some() || state.has_notification()
}

/// How long after a keystroke the decorative animation stays out of the way.
///
/// Long enough to cover a continuous typing burst, short enough that the
/// animation resumes smoothly as soon as the user pauses, so a draft left in the
/// composer does not permanently downgrade the animation.
const COMPOSING_ANIMATION_BACKOFF: Duration = Duration::from_millis(600);

/// Whether the user is actively typing right now.
///
/// A non-empty composer alone is not enough: a draft can sit there for minutes,
/// and downgrading the animation for all of it would be a visible regression for
/// no latency benefit.
fn actively_composing(state: &dyn TuiState) -> bool {
    !state.input().is_empty()
        && state
            .time_since_user_interaction()
            .is_some_and(|since| since < COMPOSING_ANIMATION_BACKOFF)
}

/// Tick cadence for the current state, with both the performance policy and
/// "is the decorative animation actually on screen" stated explicitly.
///
/// `animation_on_screen` exists because the renderer can drop the animation
/// after the scheduler has decided the screen wants one (full-screen overlay, or
/// a terminal too short to reserve donut rows). Pacing the loop at animation FPS
/// in that case costs a full frame per tick and paints nothing; see
/// [`idle_donut_paces_redraws`].
pub(crate) fn redraw_interval_with_policy_and_animation(
    state: &dyn TuiState,
    policy: &crate::perf::TuiPerfPolicy,
    animation_on_screen: bool,
) -> Duration {
    let animation_interval = fps_to_duration(policy.animation_fps);
    let fast_interval = fps_to_duration(policy.redraw_fps);

    // A retained/collapsing reasoning trace used to need animation cadence here;
    // anchored traces are static transcript messages now. The tail-follow
    // catch-up slide still needs smooth frames and must skip the deep-idle
    // short-circuits below.
    if ui::tail_catchup_active() {
        return match policy.tier {
            crate::perf::PerformanceTier::Minimal => fast_interval,
            _ => animation_interval,
        };
    }

    // The elastic overscroll line shows a live `(overscroll x.x)` countdown that
    // depletes over ~1.5s. Without a dedicated branch it falls through to the
    // 250ms idle cadence and ticks in coarse, steppy jumps. Drive it at the
    // smooth animation cadence so the countdown reads as continuous. A line
    // pinned on by config has no countdown (`remaining` is None) and must not
    // pin the redraw loop at animation cadence forever.
    if state.chat_overscroll_remaining().is_some() {
        return match policy.tier {
            crate::perf::PerformanceTier::Minimal => fast_interval,
            _ => animation_interval,
        };
    }

    // While the terminal is backgrounded (FocusLost), an idle session has nothing
    // worth a fast tick: decorative animations are paused and the run loop only
    // repaints throttled idle frames. Use the slow deep-idle interval so the
    // event loop sleeps instead of spinning on shared-server bus chatter. Sessions
    // with live output keep a responsive cadence below.
    if !state.client_focused()
        && !state.is_processing()
        && state.streaming_text().is_empty()
        && !state.has_pending_mouse_scroll_animation()
        && !state.copy_selection_edge_autoscroll_active()
        && !state.remote_startup_phase_active()
        && !rate_limit_countdown_redraw_active(state)
        && crate::build::read_build_progress().is_none()
    {
        return REDRAW_DEEP_IDLE;
    }

    let deep_idle = deep_idle_dormant(state);

    if deep_idle
        && !state.is_processing()
        && state.streaming_text().is_empty()
        && !state.has_pending_mouse_scroll_animation()
        && !state.copy_selection_edge_autoscroll_active()
        && !state.remote_startup_phase_active()
        && !rate_limit_countdown_redraw_active(state)
        && !cache_cold_countdown_redraw_active(state)
        && crate::build::read_build_progress().is_none()
        && !swarm_spinner_redraw_active(state)
        && !session_picker_spinner_redraw_active(state)
    {
        return REDRAW_DEEP_IDLE;
    }

    if idle_donut_paces_redraws(state, policy, animation_on_screen) {
        // While the user is actively typing, the input line matters and the
        // decoration does not. A 60fps donut means a keystroke can land behind an
        // in-flight animation frame, so typing into a fresh session felt sluggish
        // exactly when responsiveness is most visible. Keep animating (the donut
        // still moves, just slower) at a cadence that leaves the loop free for
        // keystrokes, and return to full smoothness as soon as typing pauses.
        if actively_composing(state) {
            return REDRAW_IDLE;
        }
        return match policy.tier {
            crate::perf::PerformanceTier::Minimal => fast_interval,
            // Decorative only: capped, because the cost is linear in frame rate
            // and the perceived smoothness is not. See
            // `DECORATIVE_ANIMATION_FPS_CAP`.
            _ => decorative_animation_interval(policy),
        };
    }

    if full_frame_status_animation_active_with_policy(state, policy) {
        return match policy.tier {
            crate::perf::PerformanceTier::Minimal => REDRAW_IDLE,
            _ => fast_interval,
        };
    }

    if primary_status_spinner_needs_full_redraw_with_policy(state, policy) {
        return match policy.tier {
            crate::perf::PerformanceTier::Minimal => REDRAW_IDLE,
            _ => fast_interval,
        };
    }

    // Swarm status spinners animate at a fixed 12.5 fps off the wall clock.
    // Streaming/scroll branches below already repaint faster than this, but
    // both the quiet-coordinator case and the processing-without-streaming
    // case (which otherwise idles at the 1s passive-liveness cadence) need
    // this to keep agent spinners smooth while the swarm works.
    if (swarm_spinner_redraw_active(state) || session_picker_spinner_redraw_active(state))
        && state.streaming_text().is_empty()
        && !state.has_pending_mouse_scroll_animation()
    {
        return match policy.tier {
            // Minimal tier drops decorative animation; a liveness-rate tick
            // still advances the glyph so agents never look frozen.
            crate::perf::PerformanceTier::Minimal => REDRAW_PASSIVE_LIVENESS,
            _ => REDRAW_SWARM_SPINNER,
        };
    }

    if !state.has_pending_mouse_scroll_animation()
        && state.streaming_text().is_empty()
        && (state.is_processing() || rate_limit_countdown_redraw_active(state))
    {
        return REDRAW_PASSIVE_LIVENESS;
    }

    if state.is_processing()
        || !state.streaming_text().is_empty()
        || state.has_pending_mouse_scroll_animation()
        || state.copy_selection_edge_autoscroll_active()
        || rate_limit_countdown_redraw_active(state)
    {
        return match policy.tier {
            crate::perf::PerformanceTier::Minimal => REDRAW_IDLE,
            _ => fast_interval,
        };
    }

    // Static text chrome only needs a tick fast enough to retire it, never the
    // animation cadence. Keep this below the animated branches above so live
    // output still wins.
    if static_text_chrome_active(state) {
        return REDRAW_IDLE;
    }

    if state.remote_startup_phase_active() {
        return REDRAW_REMOTE_STARTUP;
    }

    if deep_idle {
        REDRAW_DEEP_IDLE
    } else {
        REDRAW_IDLE
    }
}

/// Policy-only cadence: what the schedule would be if the decorative animation
/// were on screen.
///
/// This is the right entry point for reasoning about (and testing) the cadence
/// *policy* without depending on renderer state. The live loop must use
/// [`redraw_interval`], which additionally accounts for the animation having
/// been dropped by layout.
///
/// Only tests call this today (the run loop needs the live variant), hence the
/// allow: it documents the policy/liveness split rather than being dead weight.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn redraw_interval_with_policy(
    state: &dyn TuiState,
    policy: &crate::perf::TuiPerfPolicy,
) -> Duration {
    redraw_interval_with_policy_and_animation(state, policy, true)
}

pub(crate) fn redraw_interval(state: &dyn TuiState) -> Duration {
    let policy = crate::perf::tui_policy();
    redraw_interval_with_policy_and_animation(state, &policy, animation_on_screen_now())
}

pub(crate) fn periodic_redraw_required(state: &dyn TuiState) -> bool {
    periodic_redraw_required_inner(state, true)
}

/// Same as [`periodic_redraw_required`] but ignoring the decorative idle
/// animation. The run loop uses this to tell "real state changed, draw a full
/// frame" apart from "only the animation advanced", which it repaints with an
/// animation-only partial update instead of a full frame.
pub(crate) fn periodic_redraw_required_excluding_idle_animation(state: &dyn TuiState) -> bool {
    periodic_redraw_required_inner(state, false)
}

fn periodic_redraw_required_inner(state: &dyn TuiState, include_idle_animation: bool) -> bool {
    let policy = crate::perf::tui_policy();

    let deep_idle = deep_idle_dormant(state);

    if deep_idle
        && !state.is_processing()
        && state.streaming_text().is_empty()
        && !state.has_pending_mouse_scroll_animation()
        && !state.copy_selection_edge_autoscroll_active()
        // Only the elastic countdown needs ticks; a config-pinned line is static.
        && state.chat_overscroll_remaining().is_none()
        && !state.remote_startup_phase_active()
        && !rate_limit_countdown_redraw_active(state)
        && !cache_cold_countdown_redraw_active(state)
        && crate::build::read_build_progress().is_none()
        && !swarm_spinner_redraw_active(state)
        && !session_picker_spinner_redraw_active(state)
    {
        return false;
    }

    let animation_paces_redraws =
        idle_donut_paces_redraws(state, &policy, animation_on_screen_now());
    if include_idle_animation && animation_paces_redraws {
        return true;
    }

    if full_frame_status_animation_active_with_policy(state, &policy) {
        record_full_frame_redraw_reason("status_animation");
        return true;
    }

    if swarm_spinner_redraw_active(state) {
        record_full_frame_redraw_reason("swarm_spinner");
        return true;
    }

    if session_picker_spinner_redraw_active(state) {
        record_full_frame_redraw_reason("session_picker_spinner");
        return true;
    }

    if let Some(reason) = live_activity_redraw_reason(state) {
        record_full_frame_redraw_reason(reason);
        return true;
    }

    false
}

/// Why a tick needs a full frame beyond the decorative animation, or `None`
/// when nothing else is live.
///
/// Named rather than a bare boolean chain so `draw-stats` can report the exact
/// predicate keeping the animation on the expensive full-frame path. Without
/// this, diagnosing "the animation ticks are still doing full renders" means
/// bisecting a ten-term `||`.
fn live_activity_redraw_reason(state: &dyn TuiState) -> Option<&'static str> {
    if state.is_processing() {
        return Some("processing");
    }
    if !state.streaming_text().is_empty() {
        return Some("streaming");
    }
    if ui::tail_catchup_active() {
        return Some("tail_catchup");
    }
    if state.status_notice().is_some() {
        return Some("status_notice");
    }
    if state.learn_hint().is_some() {
        return Some("learn_hint");
    }
    if state.has_pending_mouse_scroll_animation() {
        return Some("mouse_scroll_animation");
    }
    if state.copy_selection_edge_autoscroll_active() {
        return Some("copy_autoscroll");
    }
    if state.chat_overscroll_remaining().is_some() {
        return Some("chat_overscroll");
    }
    if state.has_notification() {
        return Some("notification");
    }
    if rate_limit_countdown_redraw_active(state) {
        return Some("rate_limit_countdown");
    }
    if state.remote_startup_phase_active() {
        return Some("remote_startup");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both tests mutate the single process-wide reason slot, so they must not
    /// interleave.
    fn reason_slot_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Reasons are recorded by index into a fixed table, so a name that is not
    /// in the table would silently record nothing. Pin every reason the code
    /// actually passes so a rename cannot quietly blind the diagnostics.
    #[test]
    fn every_recorded_reason_is_in_the_reason_table() {
        let _lock = reason_slot_lock();
        for reason in [
            "processing",
            "streaming",
            "tail_catchup",
            "status_notice",
            "learn_hint",
            "mouse_scroll_animation",
            "copy_autoscroll",
            "chat_overscroll",
            "notification",
            "rate_limit_countdown",
            "remote_startup",
            "status_animation",
            "swarm_spinner",
            "session_picker_spinner",
        ] {
            assert!(
                FULL_FRAME_REDRAW_REASONS.contains(&reason),
                "{reason} is recorded but missing from FULL_FRAME_REDRAW_REASONS"
            );
            record_full_frame_redraw_reason(reason);
            assert_eq!(
                last_full_frame_redraw_reason(),
                Some(reason),
                "{reason} did not round-trip through the reason slot"
            );
        }
    }

    #[test]
    fn unknown_reasons_do_not_corrupt_the_reason_slot() {
        let _lock = reason_slot_lock();
        record_full_frame_redraw_reason("notification");
        record_full_frame_redraw_reason("not-a-real-reason");
        assert_eq!(last_full_frame_redraw_reason(), Some("notification"));
    }

    /// The static-chrome cadence has to be fast enough to retire a notice
    /// promptly (notices expire after 3s) while being far slower than the
    /// animation cadence that caused the lag.
    ///
    /// The behavioral gate (a notice must not pull a real state to animation
    /// cadence) lives in `ui_tests::basic::redraw_cadence`, which can build a
    /// full `TuiState`.
    #[test]
    fn static_chrome_cadence_retires_notices_without_animation_cost() {
        assert!(
            REDRAW_IDLE <= Duration::from_millis(250),
            "a notice must retire within a frame or two of its 3s expiry"
        );
        assert!(
            REDRAW_IDLE >= fps_to_duration(60) * 4,
            "static chrome must cost far fewer frames than animation cadence"
        );
    }
}
