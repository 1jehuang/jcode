//! Push-to-talk through an external recorder, the path terminal builds use.
//! Streams to Nari, so it uses credits.
//!
//! cargo run -p jcode-base --example voice_recorder -- <seconds> [recorder]
//!
//! With no recorder argument, auto-detects pw-record, parecord, arecord, rec,
//! or ffmpeg, exactly like the TUI. A recorder argument is any shell command
//! printing raw mono 16 kHz s16le PCM, for example a synthetic speech file:
//!   espeak-ng -w /tmp/s.wav "fix the flaky test"
//!   ... -- 4 "ffmpeg -loglevel error -re -i /tmp/s.wav -ac 1 -ar 16000 -f s16le -"
//!
//! Prints stage timings and the final transcript.
use jcode_voice::{NariEvent, NariRecording, nari_api_key, timing};
use std::{
    sync::{Arc, atomic::AtomicBool},
    time::{Duration, Instant},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // SAFETY: single-threaded before any other thread starts.
    unsafe { std::env::set_var("JCODE_VOICE_TIMING", "1") };
    let mut args = std::env::args().skip(1);
    let secs: u64 = args.next().map(|s| s.parse()).transpose()?.unwrap_or(4);
    let recorder = args.next();
    let key = nari_api_key().ok_or(jcode_voice::VoiceError::NariNotConfigured)?;
    timing::begin();
    let recording = NariRecording::start_auto(
        Arc::new(AtomicBool::new(false)),
        &key,
        recorder.as_deref().unwrap_or(""),
    )?;
    let until = Instant::now() + Duration::from_secs(secs);
    let mut peak = 0.0f32;
    let mut early = None;
    while Instant::now() < until && early.is_none() {
        peak = peak.max(recording.audio_level());
        while let Some(event) = recording.try_event() {
            match event {
                NariEvent::Transcript(t) => eprintln!("partial: {t}"),
                NariEvent::Finished(r) => early = Some(r),
                NariEvent::Started => {}
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let result = early.unwrap_or_else(|| {
        timing::release();
        recording.stop();
        loop {
            match recording.try_event() {
                Some(NariEvent::Finished(r)) => break r,
                Some(_) => {}
                None => std::thread::sleep(Duration::from_millis(2)),
            }
        }
    });
    eprintln!("peak level: {peak:.3}");
    println!("{}", result?);
    Ok(())
}
