//! Opt-in push-to-talk latency marks. Enable with `JCODE_VOICE_TIMING=1`.
//! Prints only stage names and milliseconds since the press (and since the
//! release once it happens), never audio, transcripts, or credentials.
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

static ORIGIN: Mutex<Option<Instant>> = Mutex::new(None);
static RELEASE: Mutex<Option<Instant>> = Mutex::new(None);

pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("JCODE_VOICE_TIMING").is_some_and(|v| v != "0"))
}

/// Starts a new attempt. Call at the user's press.
pub fn begin() {
    if enabled() {
        *ORIGIN.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
        *RELEASE.lock().unwrap_or_else(|e| e.into_inner()) = None;
        eprintln!("voice timing: {:>6.1} ms  press", 0.0);
    }
}

/// Marks the user's release. Later marks also report time since release.
pub fn release() {
    if enabled() {
        *RELEASE.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
        mark("release");
    }
}

/// Records a stage relative to the most recent press.
pub fn mark(stage: &str) {
    if !enabled() {
        return;
    }
    let Some(origin) = *ORIGIN.lock().unwrap_or_else(|e| e.into_inner()) else {
        return;
    };
    let press = origin.elapsed().as_secs_f64() * 1000.0;
    match *RELEASE.lock().unwrap_or_else(|e| e.into_inner()) {
        Some(release) => eprintln!(
            "voice timing: {press:>6.1} ms  (+{:>6.1} ms after release)  {stage}",
            release.elapsed().as_secs_f64() * 1000.0
        ),
        None => eprintln!("voice timing: {press:>6.1} ms  {stage}"),
    }
}
