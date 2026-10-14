//! Profiles push-to-talk startup with the real microphone and Nari. Uses credits.
//!
//! JCODE_VOICE_TIMING=1 cargo run -p jcode-base --features voice-capture \
//!     --example voice_startup_profile -- 4
//!
//! Records for N seconds (default 4) after the simulated press. Prints only
//! stage timings and transcript length, never audio or transcript text.
use jcode_voice::{NariEvent, NariRecording, nari_api_key, timing};
use std::{
    sync::{Arc, atomic::AtomicBool},
    time::{Duration, Instant},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // SAFETY: single-threaded before any other thread starts.
    unsafe { std::env::set_var("JCODE_VOICE_TIMING", "1") };
    let secs: u64 = std::env::args()
        .nth(1)
        .map(|s| s.parse())
        .transpose()?
        .unwrap_or(4);
    let key = nari_api_key().ok_or(jcode_voice::VoiceError::NariNotConfigured)?;
    timing::begin();
    let recording = NariRecording::start_cancellable(Arc::new(AtomicBool::new(false)), &key)?;
    timing::mark("start_cancellable returned");
    let until = Instant::now() + Duration::from_secs(secs);
    let mut chars = 0;
    let mut early = None;
    while Instant::now() < until && early.is_none() {
        while let Some(event) = recording.try_event() {
            match event {
                NariEvent::Transcript(t) => chars = t.chars().count(),
                NariEvent::Finished(r) => early = Some(r),
                NariEvent::Started => {}
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    if let Some(r) = early {
        // Setup or network failure before the simulated release.
        r?;
        eprintln!("finished before release");
        return Ok(());
    }
    timing::release();
    recording.stop();
    loop {
        match recording.try_event() {
            Some(NariEvent::Finished(r)) => {
                timing::mark("finished observed by caller");
                match r {
                    Ok(t) => eprintln!("transcript_chars={} (live {chars})", t.chars().count()),
                    Err(e) => return Err(e.into()),
                }
                break;
            }
            Some(_) => {}
            None => std::thread::sleep(Duration::from_millis(2)),
        }
    }
    Ok(())
}
