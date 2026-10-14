//! Microphone capture through an external recorder process.
//!
//! Terminal builds do not link a native audio stack, so they stream raw mono
//! 16 kHz signed 16-bit little-endian PCM from a recorder on stdout: PipeWire
//! `pw-record`, PulseAudio `parecord`, ALSA `arecord`, SoX `rec`, or `ffmpeg`.
//! A custom `[dictation] recorder` command may be any shell command that
//! prints that format. The recorder is started only after an explicit press,
//! and stop, cancel, drop, or the five-minute cap terminates it.
use super::{MAX_RECORDING_DURATION, PcmRecording, VoiceError};
use std::{
    io::Read,
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

/// 100 ms of 16 kHz audio per chunk, the same cadence as native capture.
const CHUNK_SAMPLES: usize = 1600;
const MAX_SAMPLES: usize = 16000 * MAX_RECORDING_DURATION.as_secs() as usize;
/// A recorder that prints nothing this long after starting is treated as
/// broken, so the next candidate can be tried.
const FIRST_AUDIO_TIMEOUT: Duration = Duration::from_secs(3);

/// One way to start a recorder that prints raw s16le mono 16 kHz PCM.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecorderCommand {
    pub program: String,
    pub args: Vec<String>,
}

impl RecorderCommand {
    fn new(program: &str, args: &[&str]) -> Self {
        Self {
            program: program.to_string(),
            args: args.iter().map(|arg| arg.to_string()).collect(),
        }
    }

    /// Short name for status text and errors.
    pub fn label(&self) -> &str {
        if self.program == SHELL {
            "custom recorder"
        } else {
            &self.program
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command
            .args(&self.args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // Own process group so terminal Ctrl+C never reaches the recorder
            // and a shell wrapper can be stopped together with its children.
            command.process_group(0);
        }
        command
    }
}

#[cfg(windows)]
const SHELL: &str = "cmd";
#[cfg(not(windows))]
const SHELL: &str = "sh";

/// Recorder candidates in preference order. A nonempty `custom` shell command
/// is the only candidate. Otherwise every known recorder found on `PATH`.
pub fn detect_recorders(custom: &str) -> Vec<RecorderCommand> {
    let custom = custom.trim();
    if !custom.is_empty() {
        #[cfg(windows)]
        let args = ["/C", custom];
        #[cfg(not(windows))]
        let args = ["-c", custom];
        return vec![RecorderCommand::new(SHELL, &args)];
    }
    known_recorders()
        .into_iter()
        .filter(|recorder| on_path(&recorder.program))
        .collect()
}

fn known_recorders() -> Vec<RecorderCommand> {
    let mut recorders = vec![
        RecorderCommand::new(
            "pw-record",
            &[
                "--raw",
                "--rate",
                "16000",
                "--channels",
                "1",
                "--format",
                "s16",
                "--latency",
                "50ms",
                "-",
            ],
        ),
        RecorderCommand::new(
            "parecord",
            &[
                "--raw",
                "--rate=16000",
                "--channels=1",
                "--format=s16le",
                "--latency-msec=50",
            ],
        ),
        RecorderCommand::new(
            "arecord",
            &[
                "-q", "-t", "raw", "-f", "S16_LE", "-r", "16000", "-c", "1", "-",
            ],
        ),
        RecorderCommand::new(
            "rec",
            &[
                "-q",
                "-t",
                "raw",
                "-r",
                "16000",
                "-c",
                "1",
                "-b",
                "16",
                "-e",
                "signed-integer",
                "-",
            ],
        ),
    ];
    // ffmpeg has no portable "default microphone" device name on macOS
    // (avfoundation needs an index) or Windows (dshow needs the device name),
    // and those platforms capture natively. Offer it only through PulseAudio.
    if cfg!(not(any(target_os = "macos", windows))) {
        recorders.push(RecorderCommand::new(
            "ffmpeg",
            &[
                "-hide_banner",
                "-loglevel",
                "error",
                "-nostdin",
                "-f",
                "pulse",
                "-i",
                "default",
                "-ac",
                "1",
                "-ar",
                "16000",
                "-f",
                "s16le",
                "-",
            ],
        ));
    }
    recorders
}

fn on_path(program: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| {
        let candidate = dir.join(program);
        candidate.is_file() || (cfg!(windows) && candidate.with_extension("exe").is_file())
    })
}

/// Blocking setup, call off the UI thread. Returns once one recorder delivers
/// audio, or the last failure.
pub(super) fn start_pcm(
    cancel: Arc<AtomicBool>,
    recorders: Vec<RecorderCommand>,
) -> Result<(PcmRecording, tokio::sync::mpsc::Receiver<Vec<i16>>), VoiceError> {
    PcmRecording::start_worker(cancel, move |tx, ready, cancel, stop, level| {
        let mut last = VoiceError::MicrophoneUnavailable;
        for recorder in &recorders {
            match run_recorder(recorder, &tx, ready, &cancel, &stop, &level) {
                Err(RecorderFailure::NoAudio(error)) => last = error,
                Err(RecorderFailure::Fatal(error)) => return Err(error),
                Ok(()) => return Ok(()),
            }
        }
        Err(last)
    })
}

enum RecorderFailure {
    /// Never produced audio. The next recorder may work.
    NoAudio(VoiceError),
    /// Failed after capture began. Audio was already committed.
    Fatal(VoiceError),
}

struct KillOnDrop(Child);
impl Drop for KillOnDrop {
    fn drop(&mut self) {
        terminate(&mut self.0);
    }
}

fn terminate(child: &mut Child) {
    if matches!(child.try_wait(), Ok(Some(_))) {
        return;
    }
    #[cfg(unix)]
    // SAFETY: signalling our own child's process group. No memory is shared.
    unsafe {
        let pgid = child.id() as libc::pid_t;
        libc::kill(-pgid, libc::SIGTERM);
    }
    let deadline = Instant::now() + Duration::from_millis(300);
    while Instant::now() < deadline {
        if matches!(child.try_wait(), Ok(Some(_))) {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    #[cfg(unix)]
    // SAFETY: as above.
    unsafe {
        libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn run_recorder(
    recorder: &RecorderCommand,
    tx: &tokio::sync::mpsc::Sender<Vec<i16>>,
    ready: &mpsc::SyncSender<Result<(), VoiceError>>,
    cancel: &AtomicBool,
    stop: &AtomicBool,
    level: &AtomicU32,
) -> Result<(), RecorderFailure> {
    use RecorderFailure::{Fatal, NoAudio};
    if cancel.load(Ordering::SeqCst) {
        return Err(Fatal(VoiceError::Cancelled));
    }
    super::timing::mark("recorder spawning");
    let mut child = KillOnDrop(
        recorder
            .command()
            .spawn()
            .map_err(|_| NoAudio(VoiceError::MicrophoneUnavailable))?,
    );
    let mut stdout = child
        .0
        .stdout
        .take()
        .ok_or(NoAudio(VoiceError::CaptureFailed))?;
    // Reads block, so a reader thread forwards bytes and the loop below stays
    // responsive to stop and cancel.
    let (bytes_tx, bytes_rx) = mpsc::sync_channel::<Vec<u8>>(64);
    thread::Builder::new()
        .name("voice-recorder-read".into())
        .spawn(move || {
            let mut buffer = [0u8; 3200];
            loop {
                match stdout.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if bytes_tx.send(buffer[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        })
        .map_err(|_| NoAudio(VoiceError::CaptureFailed))?;

    let mut pcm = PcmAssembler::default();
    let started = Instant::now();
    let mut signalled = false;
    loop {
        if cancel.load(Ordering::SeqCst) {
            return Err(Fatal(VoiceError::Cancelled));
        }
        if signalled && stop.load(Ordering::SeqCst) {
            break;
        }
        if !signalled && started.elapsed() >= FIRST_AUDIO_TIMEOUT {
            return Err(NoAudio(VoiceError::MicrophoneUnavailable));
        }
        match bytes_rx.recv_timeout(Duration::from_millis(10)) {
            Ok(bytes) => {
                if !signalled {
                    signalled = true;
                    super::timing::mark("recorder delivered first audio");
                    let _ = ready.send(Ok(()));
                }
                for chunk in pcm.push(&bytes, level) {
                    // Never block. Fail closed rather than silently lose audio.
                    if tx.try_send(chunk).is_err() {
                        return Err(Fatal(VoiceError::CaptureFailed));
                    }
                }
                if pcm.samples >= MAX_SAMPLES {
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                if !signalled {
                    return Err(NoAudio(VoiceError::MicrophoneUnavailable));
                }
                // The recorder exited on its own after capturing audio.
                break;
            }
        }
    }
    drop(child);
    // Bytes already read before the stop are part of the utterance.
    while let Ok(bytes) = bytes_rx.try_recv() {
        for chunk in pcm.push(&bytes, level) {
            if tx.try_send(chunk).is_err() {
                return Err(Fatal(VoiceError::CaptureFailed));
            }
        }
    }
    if let Some(chunk) = pcm.finish() {
        tx.try_send(chunk)
            .map_err(|_| Fatal(VoiceError::CaptureFailed))?;
    }
    level.store(0, Ordering::Relaxed);
    Ok(())
}

/// Turns a raw little-endian byte stream into bounded PCM chunks and keeps the
/// latest RMS level for the meter.
#[derive(Default)]
struct PcmAssembler {
    odd_byte: Option<u8>,
    chunk: Vec<i16>,
    samples: usize,
}

impl PcmAssembler {
    fn push(&mut self, bytes: &[u8], level: &AtomicU32) -> Vec<Vec<i16>> {
        let mut out = Vec::new();
        let mut energy = 0.0f64;
        let mut count = 0usize;
        let mut iter = bytes.iter().copied();
        let mut next_sample = |odd: &mut Option<u8>| -> Option<i16> {
            let lo = odd.take().or_else(|| iter.next())?;
            match iter.next() {
                Some(hi) => Some(i16::from_le_bytes([lo, hi])),
                None => {
                    *odd = Some(lo);
                    None
                }
            }
        };
        while let Some(sample) = next_sample(&mut self.odd_byte) {
            if self.samples >= MAX_SAMPLES {
                break;
            }
            let normalized = f64::from(sample) / 32768.0;
            energy += normalized * normalized;
            count += 1;
            self.chunk.push(sample);
            self.samples += 1;
            if self.chunk.len() >= CHUNK_SAMPLES {
                out.push(std::mem::replace(
                    &mut self.chunk,
                    Vec::with_capacity(CHUNK_SAMPLES),
                ));
            }
        }
        if count > 0 {
            let rms = (energy / count as f64).sqrt() as f32;
            level.store(rms.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
        }
        out
    }

    fn finish(&mut self) -> Option<Vec<i16>> {
        (!self.chunk.is_empty()).then(|| std::mem::take(&mut self.chunk))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn level_of(level: &AtomicU32) -> f32 {
        f32::from_bits(level.load(Ordering::Relaxed))
    }

    #[test]
    fn assembler_joins_split_samples_and_bounds_chunks() {
        let level = AtomicU32::new(0);
        let mut pcm = PcmAssembler::default();
        let sample = 16384i16.to_le_bytes();
        // A sample split across two reads must not be dropped or misaligned.
        assert!(pcm.push(&[sample[0]], &level).is_empty());
        let mut bytes = vec![sample[1]];
        for _ in 0..CHUNK_SAMPLES {
            bytes.extend_from_slice(&sample);
        }
        let chunks = pcm.push(&bytes, &level);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].len(), CHUNK_SAMPLES);
        assert!(chunks[0].iter().all(|s| *s == 16384));
        assert_eq!(pcm.finish(), Some(vec![16384]));
        assert_eq!(level_of(&level), 0.5);
    }

    #[test]
    fn assembler_stops_at_duration_cap() {
        let level = AtomicU32::new(0);
        let mut pcm = PcmAssembler {
            samples: MAX_SAMPLES - 2,
            ..Default::default()
        };
        pcm.push(&[1, 0, 2, 0, 3, 0, 4, 0], &level);
        assert_eq!(pcm.samples, MAX_SAMPLES);
        assert_eq!(pcm.finish(), Some(vec![1, 2]));
    }

    #[test]
    fn custom_recorder_is_the_only_candidate() {
        let recorders = detect_recorders("  my-mic --raw  ");
        assert_eq!(recorders.len(), 1);
        assert_eq!(recorders[0].program, SHELL);
        assert_eq!(recorders[0].args.last().unwrap(), "my-mic --raw");
        assert_eq!(recorders[0].label(), "custom recorder");
    }

    #[test]
    fn known_recorders_request_raw_mono_16k_s16le() {
        for recorder in known_recorders() {
            let args = recorder.args.join(" ");
            assert!(args.contains("16000"), "{}: {args}", recorder.program);
            assert!(
                args.contains("s16") || args.contains("S16_LE") || args.contains("signed-integer"),
                "{}: {args}",
                recorder.program
            );
        }
    }

    #[cfg(unix)]
    fn shell(script: &str) -> RecorderCommand {
        RecorderCommand::new("sh", &["-c", script])
    }

    #[cfg(unix)]
    fn drain(rx: &mut tokio::sync::mpsc::Receiver<Vec<i16>>) -> Vec<i16> {
        let mut all = Vec::new();
        while let Ok(chunk) = rx.try_recv() {
            all.extend(chunk);
        }
        all
    }

    #[cfg(unix)]
    #[test]
    fn recorder_streams_until_stop_then_is_terminated() {
        // Endless 0x01 0x00 bytes = sample value 1.
        let (recording, mut rx) = start_pcm(
            Arc::new(AtomicBool::new(false)),
            vec![shell("while :; do printf '\\001\\000\\001\\000'; done")],
        )
        .expect("recorder starts");
        thread::sleep(Duration::from_millis(100));
        let started = Instant::now();
        recording.finish().expect("clean stop");
        assert!(started.elapsed() < Duration::from_secs(2));
        let samples = drain(&mut rx);
        assert!(!samples.is_empty());
        assert!(samples.iter().all(|s| *s == 1));
        assert!(rx.try_recv().is_err(), "EOF after stop");
    }

    #[cfg(unix)]
    #[test]
    fn falls_through_broken_recorders_to_a_working_one() {
        let (recording, mut rx) = start_pcm(
            Arc::new(AtomicBool::new(false)),
            vec![
                RecorderCommand::new("jcode-definitely-missing-recorder", &[]),
                shell("exit 3"),
                shell("printf '\\002\\000\\002\\000'; sleep 5"),
            ],
        )
        .expect("third recorder works");
        thread::sleep(Duration::from_millis(50));
        recording.finish().unwrap();
        assert_eq!(drain(&mut rx), vec![2, 2]);
    }

    #[cfg(unix)]
    #[test]
    fn no_working_recorder_reports_microphone_unavailable() {
        let result = start_pcm(Arc::new(AtomicBool::new(false)), vec![shell("exit 1")]);
        assert!(matches!(result, Err(VoiceError::MicrophoneUnavailable)));
    }

    #[cfg(unix)]
    #[test]
    fn recorder_exiting_after_audio_ends_the_stream_cleanly() {
        let (recording, mut rx) = start_pcm(
            Arc::new(AtomicBool::new(false)),
            vec![shell("printf '\\003\\000'")],
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !recording.is_finished() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        assert!(recording.is_finished());
        recording.finish().unwrap();
        assert_eq!(drain(&mut rx), vec![3]);
    }
}
