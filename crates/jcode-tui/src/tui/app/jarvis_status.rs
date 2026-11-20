//! Transcript-free runtime capture status for the user's Waybar.
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub(super) struct CaptureStatus {
    path: PathBuf,
    temporary: PathBuf,
    pause: PathBuf,
    retry_token: String,
    // Only typed VoiceError (whose Display is redacted) or constant fallback text.
    failure: Option<(String, &'static str)>,
    /// Created by the companion on an Fn tap: act on recent speech.
    wake: PathBuf,
    _lease: File,
    levels: VecDeque<f32>,
    written: Option<Instant>,
    last_state: Option<(bool, String)>,
    sequence: u64,
}

impl CaptureStatus {
    pub(super) fn new() -> Result<Self> {
        let root = if std::env::var_os("JCODE_TEST_SESSION").is_some() {
            PathBuf::from(
                std::env::var_os("JCODE_HOME")
                    .context("Test voice requires isolated JCODE_HOME")?,
            )
            .join("runtime")
        } else {
            PathBuf::from(
                std::env::var_os("XDG_RUNTIME_DIR")
                    .context("Always-on voice requires XDG_RUNTIME_DIR")?,
            )
        };
        if std::env::var_os("JCODE_TEST_SESSION").is_some() {
            fs::create_dir_all(&root)?;
        }
        Self::in_directory(&root.join("jcode-jarvis"))
    }

    fn in_directory(directory: &Path) -> Result<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
            match fs::DirBuilder::new().mode(0o700).create(directory) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error.into()),
            }
            let metadata = fs::symlink_metadata(directory)?;
            anyhow::ensure!(
                metadata.is_dir()
                    && metadata.uid() == unsafe { libc::geteuid() }
                    && metadata.mode() & 0o077 == 0,
                "Unsafe Jarvis runtime directory"
            );
            let lease = OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(directory.join("capture.lock"))?;
            use std::os::fd::AsRawFd;
            // SAFETY: the descriptor is owned here and retained for the lease lifetime.
            anyhow::ensure!(
                unsafe { libc::flock(lease.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0,
                "Another always-on Jarvis client already owns the microphone"
            );
            let pid = std::process::id();
            Ok(Self {
                path: directory.join(format!("{pid}.json")),
                temporary: directory.join(format!("{pid}.tmp")),
                pause: directory.join(format!("{pid}.paused")),
                retry_token: format!("{:016x}", rand::random::<u64>()),
                failure: None,
                wake: directory.join(format!("{pid}.wake")),
                _lease: lease,
                levels: VecDeque::with_capacity(64),
                written: None,
                last_state: None,
                sequence: 0,
            })
        }
        #[cfg(not(unix))]
        anyhow::bail!("Always-on capture status currently requires Unix")
    }

    fn retry_path(&self) -> PathBuf {
        self.path
            .with_extension(format!("retry-{}", self.retry_token))
    }

    pub(super) fn take_retry(&self) -> bool {
        self.failure.is_some() && fs::remove_file(self.retry_path()).is_ok()
    }

    pub(super) fn set_failure(&mut self, error: Option<&jcode_voice::VoiceError>) {
        use jcode_voice::VoiceError::*;
        let hint = match error {
            Some(NariNotConfigured) => "Check the configured Nari API key.",
            Some(NariCreditsExhausted) => "Add Nari credits before retrying.",
            Some(NariRejected) => "Check Nari account access and voice settings.",
            Some(Http(429)) => "Wait for the provider rate limit to clear.",
            Some(Network | Timeout | Http(500..=599)) => {
                "Check the network and wait for the provider to recover."
            }
            Some(CaptureFailed | MicrophoneUnavailable) => {
                "Check the microphone, recorder and capture permissions."
            }
            Some(NotConfigured) => "Check the client sign-in and voice configuration.",
            _ => "Check the error in the Jarvis client before retrying.",
        };
        self.failure = Some((
            error
                .map(ToString::to_string)
                .unwrap_or_else(|| "Jarvis capture stopped".into()),
            hint,
        ));
        self.written = None;
    }

    /// Consume a pending manual trigger request.
    pub(super) fn take_wake(&self) -> bool {
        fs::remove_file(&self.wake).is_ok()
    }

    pub(super) fn paused(&self) -> bool {
        self.pause.exists()
    }

    pub(super) fn set_paused(&self, paused: bool) -> Result<()> {
        if paused {
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                OpenOptions::new()
                    .create(true)
                    .write(true)
                    .truncate(true)
                    .mode(0o600)
                    .custom_flags(libc::O_NOFOLLOW)
                    .open(&self.pause)?;
            }
        } else {
            match fs::remove_file(&self.pause) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }

    pub(super) fn publish(
        &mut self,
        capturing: bool,
        state: &str,
        level: f32,
        counters: Value,
    ) -> Result<()> {
        self.publish_at(capturing, state, level, counters, Instant::now())
    }

    fn publish_at(
        &mut self,
        capturing: bool,
        state: &str,
        level: f32,
        counters: Value,
        now: Instant,
    ) -> Result<()> {
        let state_changed = self
            .last_state
            .as_ref()
            .is_none_or(|(old_capture, old_state)| *old_capture != capturing || old_state != state);
        // The native renderer interpolates these measured samples using the GTK
        // frame clock. No animation-rate file I/O or second microphone is needed.
        let interval = if capturing {
            Duration::from_millis(50)
        } else {
            Duration::from_secs(1)
        };
        if !state_changed
            && self
                .written
                .is_some_and(|at| now.duration_since(at) < interval)
        {
            return Ok(());
        }
        if !capturing {
            self.levels.clear();
        }
        if self.levels.len() == 64 {
            self.levels.pop_front();
        }
        self.levels.push_back(if capturing && level.is_finite() {
            (level * 12.0).clamp(0.0, 1.0)
        } else {
            0.0
        });
        self.sequence = self.sequence.saturating_add(1);
        let mut value = json!({"pid":std::process::id(),"capturing":capturing,"state":state,"levels":self.levels,"counters":counters,"sequence":self.sequence,"sample_interval_ms":50});
        if state == "error"
            && let Some((reason, hint)) = &self.failure
        {
            value["error_reason"] = json!(reason);
            value["error_hint"] = json!(hint);
            value["retry_supported"] = json!(true);
            value["retry_token"] = json!(self.retry_token);
        }
        #[cfg(unix)]
        {
            use std::{io::Write, os::unix::fs::OpenOptionsExt};
            let mut file = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&self.temporary)?;
            file.write_all(&serde_json::to_vec(&value)?)?;
            fs::rename(&self.temporary, &self.path)?;
        }
        self.written = Some(now);
        self.last_state = Some((capturing, state.to_owned()));
        Ok(())
    }
}

impl Drop for CaptureStatus {
    fn drop(&mut self) {
        for path in [
            &self.path,
            &self.temporary,
            &self.pause,
            &self.wake,
            &self.retry_path(),
        ] {
            if let Err(error) = fs::remove_file(path)
                && error.kind() != std::io::ErrorKind::NotFound
            {
                crate::logging::warn(&format!(
                    "Could not clean up voice status {}: {error}",
                    path.display()
                ));
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[test]
    fn native_history_is_bounded_throttled_and_cleared_on_pause() {
        let temp = tempfile::tempdir().unwrap();
        let mut status = CaptureStatus::in_directory(&temp.path().join("jarvis")).unwrap();
        let start = Instant::now();
        for i in 0..100_u64 {
            status
                .publish_at(
                    true,
                    "listening",
                    if i == 99 { f32::NAN } else { 0.04 },
                    json!({}),
                    start + Duration::from_millis(i * 50),
                )
                .unwrap();
        }
        let read =
            |path: &Path| -> Value { serde_json::from_slice(&fs::read(path).unwrap()).unwrap() };
        let value = read(&status.path);
        assert_eq!(value["levels"].as_array().unwrap().len(), 64);
        assert_eq!(value["levels"][63], 0.0);
        assert_eq!(value["sample_interval_ms"], 50);
        assert_eq!(value["sequence"], 100);
        status
            .publish_at(
                true,
                "listening",
                0.2,
                json!({}),
                start + Duration::from_millis(4951),
            )
            .unwrap();
        assert_eq!(read(&status.path)["sequence"], 100);
        // Capture/state transitions are immediate, even inside the sampling interval.
        status
            .publish_at(
                false,
                "paused",
                1.0,
                json!({}),
                start + Duration::from_millis(4952),
            )
            .unwrap();
        assert_eq!(read(&status.path)["levels"], json!([0.0]));
        status
            .publish_at(
                false,
                "paused",
                1.0,
                json!({}),
                start + Duration::from_millis(5000),
            )
            .unwrap();
        assert_eq!(read(&status.path)["sequence"], 101);
        status
            .publish_at(
                false,
                "paused",
                1.0,
                json!({}),
                start + Duration::from_millis(6000),
            )
            .unwrap();
        assert_eq!(read(&status.path)["sequence"], 102);
        assert!(fs::metadata(&status.path).unwrap().len() < 8192);
    }

    #[test]
    fn error_snapshot_is_actionable_and_retry_is_single_use_and_lease_scoped() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("jcode-jarvis");
        let mut status = CaptureStatus::in_directory(&directory).unwrap();
        status.set_failure(Some(&jcode_voice::VoiceError::NariCreditsExhausted));
        status.publish(false, "error", 0., json!({})).unwrap();
        let snapshot: Value = serde_json::from_slice(&fs::read(&status.path).unwrap()).unwrap();
        assert_eq!(snapshot["state"], "error");
        assert_eq!(snapshot["retry_supported"], true);
        assert!(
            snapshot["error_reason"]
                .as_str()
                .unwrap()
                .contains("credits")
        );
        assert!(
            snapshot["error_hint"]
                .as_str()
                .unwrap()
                .contains("Add Nari credits")
        );
        let marker = status.retry_path();
        // Exercise the installed helper protocol end-to-end against an isolated runtime.
        let helper = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../scripts/waybar/jarvis-voice-status.py");
        let output = std::process::Command::new("python3")
            .arg(helper)
            .arg("--toggle")
            .env("XDG_RUNTIME_DIR", temp.path())
            .output()
            .unwrap();
        assert!(output.status.success() && output.stderr.is_empty());
        assert!(status.take_retry());
        assert!(!status.take_retry());
        drop(status);
        let mut restarted = CaptureStatus::in_directory(&directory).unwrap();
        restarted.set_failure(None);
        fs::write(marker, "").unwrap(); // late delivery from the previous indicator
        assert!(!restarted.take_retry());
        restarted.publish(false, "paused", 0., json!({})).unwrap();
        let snapshot: Value = serde_json::from_slice(&fs::read(&restarted.path).unwrap()).unwrap();
        assert!(
            snapshot.get("retry_supported").is_none() && snapshot.get("error_reason").is_none()
        );
    }

    #[test]
    fn status_is_private_exclusive_and_cleared_on_drop() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("jarvis");
        let mut status = CaptureStatus::in_directory(&directory).unwrap();
        assert!(CaptureStatus::in_directory(&directory).is_err());
        status.publish(true, "listening", 0.7, json!({})).unwrap();
        let value: Value = serde_json::from_slice(&fs::read(&status.path).unwrap()).unwrap();
        assert_eq!(value["capturing"], true);
        assert!(value.get("transcript").is_none());
        status.set_paused(true).unwrap();
        assert!(status.paused());
        status.set_paused(false).unwrap();
        assert!(!status.paused());
        let path = status.path.clone();
        drop(status);
        assert!(!path.exists());
        assert!(CaptureStatus::in_directory(&directory).is_ok());
    }
}
