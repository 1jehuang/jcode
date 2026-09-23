//! Profiles push-to-talk startup with the real microphone and Nari. Uses credits.
//!
//! JCODE_VOICE_TIMING=1 cargo run -p jcode-base --features voice-capture \
//!     --example voice_startup_profile -- 4
//!
//! Records for N seconds (default 4) after the simulated press. Prints only
//! stage timings and transcript length, never audio or transcript text.
use jcode_base::voice::{NariEvent, NariRecording, nari_api_key, timing};
use std::{
    sync::{Arc, atomic::AtomicBool},
    time::{Duration, Instant},
};

fn main() {
    // SAFETY: single-threaded before any other thread starts.
    unsafe { std::env::set_var("JCODE_VOICE_TIMING", "1") };
    let secs: u64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(4);
    let key = nari_api_key().expect("Nari key not configured");
    timing::begin();
    let recording = NariRecording::start_cancellable(Arc::new(AtomicBool::new(false)), &key)
        .expect("start failed");
    timing::mark("start_cancellable returned");
    let until = Instant::now() + Duration::from_secs(secs);
    let mut chars = 0;
    while Instant::now() < until {
        while let Some(event) = recording.try_event() {
            if let NariEvent::Transcript(t) = event {
                chars = t.chars().count();
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    timing::mark("stop");
    recording.stop();
    loop {
        match recording.try_event() {
            Some(NariEvent::Finished(r)) => {
                timing::mark("finished");
                match r {
                    Ok(t) => eprintln!("transcript_chars={} (live {chars})", t.chars().count()),
                    Err(e) => eprintln!("error: {e}"),
                }
                break;
            }
            Some(_) => {}
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}
