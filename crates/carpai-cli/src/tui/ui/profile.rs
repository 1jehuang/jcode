use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

#[derive(Default)]
struct RenderProfile {
    frames: u64,
    total: Duration,
    prepare: Duration,
    draw: Duration,
    last_log: Option<Instant>,
}

static PROFILE_STATE: OnceLock<Mutex<RenderProfile>> = OnceLock::new();

fn profile_state() -> &'static Mutex<RenderProfile> {
    PROFILE_STATE.get_or_init(|| Mutex::new(RenderProfile::default()))
}

pub(super) fn profile_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("JCODE_TUI_PROFILE").is_ok())
}

pub(super) fn record_profile(prepare: Duration, draw: Duration, total: Duration) {
    let mut state = match profile_state().lock() {
        Ok(s) => s,
        Err(poisoned) => poisoned.into_inner(),
    };
    state.frames += 1;
    state.prepare += prepare;
    state.draw += draw;
    state.total += total;

    let now = Instant::now();
    let should_log = match state.last_log {
        Some(last) => now.duration_since(last) >= Duration::from_secs(1),
        None => true,
    };
    if should_log && state.frames > 0 {
        let frames = state.frames as f64;
        let avg_prepare = state.prepare.as_secs_f64() * 1000.0 / frames;
        let avg_draw = state.draw.as_secs_f64() * 1000.0 / frames;
        let avg_total = state.total.as_secs_f64() * 1000.0 / frames;
        crate::logging::info(&format!(
            "TUI perf: {:.1} fps | prepare {:.2}ms | draw {:.2}ms | total {:.2}ms",
            frames, avg_prepare, avg_draw, avg_total
        ));
        state.frames = 0;
        state.prepare = Duration::from_secs(0);
        state.draw = Duration::from_secs(0);
        state.total = Duration::from_secs(0);
        state.last_log = Some(now);
    }
}
