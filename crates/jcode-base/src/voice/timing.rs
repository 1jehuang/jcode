//! Opt-in startup latency marks for push-to-talk. Enable with
//! `JCODE_VOICE_TIMING=1`. Prints only stage names and milliseconds since the
//! press, never audio, transcripts, or credentials.
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

static ORIGIN: Mutex<Option<Instant>> = Mutex::new(None);

pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("JCODE_VOICE_TIMING").is_some_and(|v| v != "0"))
}

/// Starts a new attempt. Call at the user's press.
pub fn begin() {
    if enabled() {
        *ORIGIN.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
        eprintln!("voice timing: {:>6.1} ms  press", 0.0);
    }
}

/// Records a stage relative to the most recent press.
pub fn mark(stage: &str) {
    if !enabled() {
        return;
    }
    if let Some(origin) = *ORIGIN.lock().unwrap_or_else(|e| e.into_inner()) {
        eprintln!(
            "voice timing: {:>6.1} ms  {stage}",
            origin.elapsed().as_secs_f64() * 1000.0
        );
    }
}
