//! Explicit, microphone-free acceptance runner for caller-provided mono 16 kHz
//! signed little-endian PCM16. May incur Nari usage charges. No audio, transcript,
//! or credential content is printed or persisted by this example.
//!
//! cargo run -p jcode-base --no-default-features --example nari_pcm -- --live sample.pcm
//!
//! Uses NARI_API_KEY or the existing private nari.env configuration. For isolated
//! acceptance, set JCODE_HOME and JCODE_RUNTIME_DIR to a scratch directory and
//! configure NARI_API_KEY through your normal secret delivery mechanism.
use jcode_base::voice::{NariEvent, NariSession, VoiceError, nari_api_key, nari_pcm_channel};
use std::{
    io::Read,
    sync::{Arc, atomic::AtomicBool},
    time::{Duration, Instant},
};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), VoiceError> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 2 || args[0] != "--live" {
        eprintln!("Usage: nari_pcm --live path-to-mono16k-s16le.pcm (uses Nari credits)");
        return Err(VoiceError::InvalidAudio);
    }
    let mut bytes = Vec::new();
    let limit = 16000 * 2 * 300;
    std::fs::File::open(&args[1])
        .map_err(|_| VoiceError::InvalidAudio)?
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| VoiceError::InvalidAudio)?;
    if bytes.is_empty() || bytes.len() as u64 > limit || bytes.len() % 2 != 0 {
        return Err(VoiceError::InvalidAudio);
    }
    let key = nari_api_key().ok_or(VoiceError::NariNotConfigured)?;
    let start = Instant::now();
    let session = NariSession::connect(&key, Arc::new(AtomicBool::new(false))).await?;
    drop(key);
    println!("setup_ms={}", start.elapsed().as_millis());
    let audio_start = Instant::now();
    let (tx, rx) = nari_pcm_channel();
    let sender = tokio::spawn(async move {
        for chunk in bytes.chunks(3200) {
            let samples = chunk
                .as_chunks::<2>()
                .0
                .iter()
                .map(|s| i16::from_le_bytes([s[0], s[1]]))
                .collect();
            if tx.send(samples).await.is_err() {
                break;
            }
            tokio::time::sleep(Duration::from_secs_f64(chunk.len() as f64 / 32000.0)).await;
        }
        let eof = Instant::now();
        drop(tx);
        eof
    });
    let mut revisions = 0usize;
    let result = session
        .run(rx, |event| {
            if let NariEvent::Transcript(_) = event {
                revisions += 1;
                if revisions == 1 {
                    println!(
                        "first_revision_from_audio_ms={}",
                        audio_start.elapsed().as_millis()
                    );
                }
            }
        })
        .await;
    let eof = sender.await.map_err(|_| VoiceError::CaptureFailed)?;
    let text = result?;
    println!(
        "post_eof_ms={} revisions={} transcript_chars={}",
        eof.elapsed().as_millis(),
        revisions,
        text.chars().count()
    );
    Ok(())
}
