//! Global push-to-talk without Jcode Desktop (Linux).
//!
//! Holding the Copilot key while a Jcode CLI terminal is focused records the
//! microphone, transcribes through Nari, and sends the text to that session,
//! exactly like Desktop's CLI capture. Opt-in: needs `[dictation]
//! global_devices` (or the keyboards picked in Desktop, `[desktop.voice]
//! global_devices`) and a Nari key.
//!
//! When Jcode Desktop is running it shares the same routing registry
//! (`$XDG_RUNTIME_DIR/jcode-global-voice-v1`), and the most recently focused
//! eligible registrant wins a press. The companion registers as eligible only
//! while a Jcode CLI window is focused, so Desktop keeps its own windows.
//!
//! Never records without a visible indicator: a desktop notification is shown
//! first, and recording is refused if it cannot be shown.
//!
//! With `[companion] jarvis_fn_key`, a bare hold of the laptop Fn key records
//! for the running always-on Jarvis session instead, from any window and
//! without changing focus. Its waveform appears in that Jarvis client, which
//! also feeds the levels to the Waybar Jarvis indicator. With
//! `jarvis_fn_launch` (default on), an Fn hold while none runs opens one in a
//! new terminal, records meanwhile behind a notification, and sends the words
//! once the new Jarvis has registered its session.

use jcode_base::protocol::VoiceActivity;
use jcode_voice::global::hotkey::{Edge, Listener};
use jcode_voice::{NariEvent, NariRecording, VoiceError};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const IDLE_POLL: Duration = Duration::from_millis(40);
const HELD_POLL: Duration = Duration::from_millis(15);
/// Compositor focus lookups are subprocesses, so refresh them less often.
const FOCUS_REFRESH: Duration = Duration::from_millis(400);
const SEND_TIMEOUT: Duration = Duration::from_secs(5);
/// Waveform update cadence to the focused TUI, matching its own meter tick.
const WAVEFORM_TICK: Duration = Duration::from_millis(80);
/// Enough samples to fill the TUI composer's waveform row (up to 48 cells).
const WAVEFORM_HISTORY: usize = 48;
const ACTIVITY_TIMEOUT: Duration = Duration::from_millis(500);
/// How long a freshly opened Jarvis has to register before its pending words
/// are dropped, and before another Fn hold may open a second one.
const JARVIS_STARTUP: Duration = Duration::from_secs(30);
/// How often a pending capture looks for the Jarvis it opened.
const JARVIS_POLL: Duration = Duration::from_millis(400);

/// Which visible indicators companion push-to-talk uses.
#[derive(Clone, Copy, Debug)]
pub struct VoiceIndicators {
    /// Desktop notifications (`notify-send`).
    pub notifications: bool,
    /// Live waveform in the focused Jcode CLI's status line.
    pub waveform: bool,
    /// Bare Fn hold talks to the always-on Jarvis session.
    pub jarvis_fn_key: bool,
    /// evdev code of that key.
    pub jarvis_key: usize,
    /// Fn opens an always-on Jarvis when none is running.
    pub jarvis_fn_launch: bool,
}

/// Config name to evdev code. Unknown names fall back to Fn.
pub fn jarvis_key_code(name: &str) -> usize {
    use jcode_voice::global::hotkey::{KEY_FN, KEY_RIGHTSHIFT};
    match name
        .trim()
        .to_ascii_lowercase()
        .replace(['_', '-', ' '], "")
        .as_str()
    {
        "rightshift" | "rshift" => KEY_RIGHTSHIFT,
        _ => KEY_FN,
    }
}

pub fn spawn(indicators: VoiceIndicators) {
    let devices = jcode_base::config::config().global_voice_devices();
    if devices.is_empty() {
        return;
    }
    if std::env::var("JCODE_COMPANION_VOICE").as_deref() == Ok("0") {
        return;
    }
    if !indicators.notifications && !indicators.waveform {
        eprintln!(
            "jcode companion: global voice off, it needs a visible indicator \
             ([companion] voice_notifications or voice_waveform)"
        );
        return;
    }
    let mut listener = match Listener::new(&devices) {
        Ok(listener) => listener,
        Err(error) => {
            jcode_base::logging::warn(&format!("companion voice unavailable: {error}"));
            eprintln!("jcode companion: global voice unavailable: {error}");
            return;
        }
    };
    listener.set_fn_trigger_key(indicators.jarvis_fn_key, indicators.jarvis_key);
    eprintln!(
        "jcode companion: global voice on {} device(s){}",
        devices.len(),
        if indicators.jarvis_fn_key {
            if indicators.jarvis_key == jcode_voice::global::hotkey::KEY_RIGHTSHIFT {
                ", Right Shift talks to Jarvis"
            } else {
                ", Fn talks to Jarvis"
            }
        } else {
            ""
        }
    );
    if let Err(error) = std::thread::Builder::new()
        .name("jcode-companion-voice".into())
        .spawn(move || run(listener, indicators))
    {
        eprintln!("jcode companion: cannot start global voice worker: {error}");
    }
}

/// Which key started a capture. Each key's edges only end its own capture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    /// Copilot key, to the focused Jcode CLI.
    Copilot,
    /// Fn key, to the always-on Jarvis session.
    JarvisFn,
}

struct Capture {
    source: Source,
    /// Target session. `None` while the Jarvis this capture opened starts.
    session: Option<String>,
    /// Show the waveform once a pending session appears.
    waveform: bool,
    next_target_poll: Instant,
    cancel: Arc<AtomicBool>,
    recording: NariRecording,
    indicator: Indicator,
    activity: Option<ActivityReporter>,
    started: Instant,
    levels: std::collections::VecDeque<f32>,
    live: String,
    next_tick: Instant,
    pending_final: Option<Result<String, VoiceError>>,
    _suppress: Option<std::fs::File>,
}

fn run(mut listener: Listener, indicators: VoiceIndicators) {
    let mut focused: Option<String> = None;
    let mut next_focus = Instant::now();
    let mut capture: Option<Capture> = None;
    // When this companion last opened a Jarvis that may still be starting.
    let mut launched: Option<Instant> = None;
    loop {
        if capture.is_none() && Instant::now() >= next_focus {
            next_focus = Instant::now() + FOCUS_REFRESH;
            focused = match jcode_voice::dictation::focused_cli_session() {
                Ok(session) => session,
                Err(error) => {
                    jcode_base::logging::warn(&format!(
                        "companion voice: cannot inspect focused CLI session: {error:#}"
                    ));
                    None
                }
            };
            // Eligible and "focused" only while a Jcode CLI is in front, so a
            // focused Desktop window wins its own presses.
            listener.set_eligible(focused.is_some());
            listener.set_active(focused.is_some());
        }
        for edge in listener.poll() {
            match edge {
                Edge::Press | Edge::Tap => {
                    if capture.is_none()
                        && let Some(session) = focused.clone()
                    {
                        capture = start(Source::Copilot, Some(session), indicators);
                    }
                }
                Edge::FnTap => {
                    if capture.is_none() {
                        fn_tap(indicators);
                    }
                }
                Edge::FnPress => {
                    if capture.is_none() {
                        match crate::jarvis_target::live_jarvis() {
                            Some(target) => {
                                launched = None;
                                capture = start(Source::JarvisFn, Some(target.session), indicators);
                            }
                            None if indicators.jarvis_fn_launch => {
                                if ensure_jarvis_launching(&mut launched) {
                                    capture = start(Source::JarvisFn, None, indicators);
                                }
                            }
                            None => {
                                Indicator::new(indicators.notifications)
                                    .show("No always-on Jarvis is running");
                            }
                        }
                    }
                }
                Edge::Release | Edge::FnRelease => {
                    if capture
                        .as_ref()
                        .is_some_and(|active| active.source == edge_source(edge))
                        && let Some(active) = capture.take()
                    {
                        finish(active);
                    }
                }
                Edge::Cancel | Edge::FnCancel => {
                    if capture
                        .as_ref()
                        .is_some_and(|active| active.source == edge_source(edge))
                        && let Some(active) = capture.take()
                    {
                        active.cancel.store(true, Ordering::SeqCst);
                        active.indicator.show("Voice cancelled");
                        if let Some(reporter) = &active.activity {
                            reporter.report(VoiceActivity::Idle);
                        }
                    }
                }
                // Shift+Copilot opens a Desktop voice window. Without
                // Desktop there is nothing to open.
                Edge::Spawn => {}
            }
        }
        if let Some(active) = capture.as_mut() {
            active.tick_waveform();
        }
        std::thread::sleep(if capture.is_some() {
            HELD_POLL
        } else {
            IDLE_POLL
        });
    }
}

fn edge_source(edge: Edge) -> Source {
    match edge {
        Edge::FnPress | Edge::FnRelease | Edge::FnCancel | Edge::FnTap => Source::JarvisFn,
        _ => Source::Copilot,
    }
}

/// Fn tap: ask the running Jarvis to act on what it just heard.
fn fn_tap(indicators: VoiceIndicators) {
    let indicator = Indicator::new(indicators.notifications);
    let Some(target) = crate::jarvis_target::live_jarvis() else {
        // Nothing has been listening, so there is nothing to send.
        return;
    };
    match crate::jarvis_target::request_wake(target.pid) {
        Ok(()) => {
            indicator.show("Jarvis: sending what it heard");
        }
        Err(error) => {
            eprintln!("jcode companion: cannot wake Jarvis: {error:#}");
            indicator.show("Couldn't reach Jarvis");
        }
    }
}

/// Open an always-on Jarvis unless one this companion opened is still
/// starting. Returns whether one is on its way.
fn ensure_jarvis_launching(launched: &mut Option<Instant>) -> bool {
    if launched.is_some_and(|at| at.elapsed() < JARVIS_STARTUP) {
        return true;
    }
    let extra = jcode_base::config::config()
        .companion
        .jarvis_fn_launch_args
        .clone();
    match crate::jarvis_launch::launch(&extra) {
        Ok(true) => {
            eprintln!("jcode companion: opened an always-on Jarvis for Fn");
            *launched = Some(Instant::now());
            true
        }
        Ok(false) => {
            Indicator::new(true).show("No terminal found to open Jarvis");
            false
        }
        Err(error) => {
            eprintln!("jcode companion: cannot open Jarvis: {error:#}");
            Indicator::new(true).show("Couldn't open Jarvis");
            false
        }
    }
}

fn start(source: Source, session: Option<String>, indicators: VoiceIndicators) -> Option<Capture> {
    if !block_on(jcode_voice::global::session::permitted()) {
        eprintln!("jcode companion: voice denied by session check");
        return None;
    }
    let Some(key) = jcode_voice::nari_api_key() else {
        Indicator::new(true).show("Voice not configured (NARI_API_KEY)");
        return None;
    };
    // Prove a visible indicator exists before opening the microphone: the
    // focused TUI's waveform, or a desktop notification. A Jarvis that is
    // still starting has no waveform yet, so it needs the notification.
    let activity = session
        .clone()
        .filter(|_| indicators.waveform)
        .map(ActivityReporter::new);
    let waveform_shown = activity.as_ref().is_some_and(|reporter| {
        reporter.report_now(VoiceActivity::Recording {
            elapsed_ms: 0,
            levels: Vec::new(),
            live: String::new(),
        })
    });
    let indicator = Indicator::new(indicators.notifications);
    let notified = indicators.notifications
        && indicator.show(match (source, session.is_some()) {
            (Source::Copilot, _) => "Listening for Jcode…",
            (Source::JarvisFn, true) => "Listening for Jarvis…",
            (Source::JarvisFn, false) => "Opening Jarvis, listening…",
        });
    if !waveform_shown && !notified {
        eprintln!("jcode companion: no visible voice indicator, not recording");
        return None;
    }
    let cancel = Arc::new(AtomicBool::new(false));
    let suppress = crate::jarvis_target::acquire_dictation_suppress();
    let recorder = jcode_base::config::config().dictation.recorder.clone();
    match NariRecording::start_auto(cancel.clone(), &key, &recorder) {
        Ok(recording) => Some(Capture {
            source,
            session,
            waveform: indicators.waveform,
            next_target_poll: Instant::now() + JARVIS_POLL,
            cancel,
            recording,
            indicator,
            activity,
            started: Instant::now(),
            levels: std::collections::VecDeque::with_capacity(WAVEFORM_HISTORY),
            live: String::new(),
            next_tick: Instant::now(),
            pending_final: None,
            _suppress: suppress,
        }),
        Err(error) => {
            indicator.show(&failure_label(&error));
            if let Some(reporter) = &activity {
                reporter.report(VoiceActivity::Idle);
            }
            None
        }
    }
}

/// Key-up: stop the microphone, wait for the final text, deliver it.
fn finish(mut capture: Capture) {
    capture.indicator.show("Transcribing…");
    capture.recording.stop();
    if let Some(reporter) = &capture.activity {
        reporter.report(VoiceActivity::Transcribing {
            live: capture.live.clone(),
        });
    }
    if let Err(error) = std::thread::Builder::new()
        .name("jcode-companion-voice-send".into())
        .spawn(move || {
            let result = wait_final(&mut capture);
            let label = match result {
                Ok(text) if text.trim().is_empty() => "No speech detected".to_string(),
                Ok(text) => match capture.session.clone().or_else(|| {
                    capture.indicator.show("Waiting for Jarvis to start…");
                    wait_for_jarvis(capture.started + JARVIS_STARTUP)
                }) {
                    None => "Jarvis didn't start in time".to_string(),
                    Some(_) if !block_on(jcode_voice::global::session::permitted()) => {
                        "Voice denied (session locked)".to_string()
                    }
                    Some(session) => {
                        match jcode_voice::dictation::send_transcript_blocking(
                            text.trim(),
                            jcode_base::protocol::TranscriptMode::Send,
                            Some(session),
                            SEND_TIMEOUT,
                        ) {
                            Ok(()) => match capture.source {
                                Source::Copilot => "Sent to Jcode".to_string(),
                                Source::JarvisFn => "Sent to Jarvis".to_string(),
                            },
                            Err(error) => {
                                eprintln!("jcode companion: voice delivery failed: {error:#}");
                                "Couldn't reach Jcode".to_string()
                            }
                        }
                    }
                },
                Err(error) => failure_label(&error),
            };
            capture.indicator.show(&label);
            if let Some(reporter) = &capture.activity {
                reporter.report(VoiceActivity::Idle);
            }
        })
    {
        eprintln!("jcode companion: cannot start voice delivery worker: {error}");
    }
}

impl Capture {
    /// Once the Jarvis this capture opened registers, target it and show the
    /// waveform there (which also hands its microphone to push-to-talk).
    fn attach_pending_jarvis(&mut self) {
        if self.session.is_some() || Instant::now() < self.next_target_poll {
            return;
        }
        self.next_target_poll = Instant::now() + JARVIS_POLL;
        let Some(target) = crate::jarvis_target::live_jarvis() else {
            return;
        };
        self.indicator.show("Listening for Jarvis…");
        if self.waveform {
            self.activity = Some(ActivityReporter::new(target.session.clone()));
        }
        self.session = Some(target.session);
    }

    /// Sample the microphone level and push the waveform to the focused TUI.
    fn tick_waveform(&mut self) {
        self.attach_pending_jarvis();
        let Some(reporter) = &self.activity else {
            return;
        };
        let now = Instant::now();
        if now < self.next_tick {
            return;
        }
        self.next_tick = now + WAVEFORM_TICK;
        if self.levels.len() == WAVEFORM_HISTORY {
            self.levels.pop_front();
        }
        self.levels.push_back(self.recording.audio_level());
        // Live text: keep Finished for `wait_final`, which drains after stop.
        while let Some(event) = self.recording.try_event() {
            match event {
                NariEvent::Transcript(text) => self.live = text,
                NariEvent::Started => {}
                NariEvent::Finished(result) => {
                    self.pending_final = Some(result);
                    break;
                }
            }
        }
        reporter.report(VoiceActivity::Recording {
            elapsed_ms: self.started.elapsed().as_millis() as u64,
            levels: self.levels.iter().copied().collect(),
            live: self.live.clone(),
        });
    }
}

/// Sends activity updates off the key-polling thread, coalescing to the
/// latest state so a slow server never delays key handling.
struct ActivityReporter {
    session: String,
    tx: mpsc::Sender<VoiceActivity>,
}

impl ActivityReporter {
    fn new(session: String) -> Self {
        let (tx, rx) = mpsc::channel::<VoiceActivity>();
        let target = session.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("jcode-companion-voice-activity".into())
            .spawn(move || {
                while let Ok(mut activity) = rx.recv() {
                    while let Ok(newer) = rx.try_recv() {
                        // Never coalesce away a terminal state change.
                        if matches!(newer, VoiceActivity::Recording { .. })
                            && !matches!(activity, VoiceActivity::Recording { .. })
                            && let Err(error) = send_activity(&target, activity)
                        {
                            jcode_base::logging::debug(&format!(
                                "Cannot send companion voice activity: {error}"
                            ));
                        }
                        activity = newer;
                    }
                    if let Err(error) = send_activity(&target, activity) {
                        jcode_base::logging::debug(&format!(
                            "Cannot send companion voice activity: {error}"
                        ));
                    }
                }
            })
        {
            jcode_base::logging::warn(&format!(
                "Cannot start companion voice activity worker: {error}"
            ));
        }
        Self { session, tx }
    }

    fn report(&self, activity: VoiceActivity) {
        if let Err(error) = self.tx.send(activity) {
            jcode_base::logging::debug(&format!(
                "Companion voice activity worker disconnected: {error}"
            ));
        }
    }

    /// Synchronous send. Returns whether the server accepted it.
    fn report_now(&self, activity: VoiceActivity) -> bool {
        send_activity(&self.session, activity).is_ok()
    }
}

fn send_activity(session: &str, activity: VoiceActivity) -> anyhow::Result<()> {
    jcode_voice::dictation::send_voice_activity_blocking(session, activity, ACTIVITY_TIMEOUT)
}

/// Poll until an always-on Jarvis registers or `deadline` passes.
fn wait_for_jarvis(deadline: Instant) -> Option<String> {
    loop {
        if let Some(target) = crate::jarvis_target::live_jarvis() {
            return Some(target.session);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(JARVIS_POLL);
    }
}

fn wait_final(capture: &mut Capture) -> Result<String, VoiceError> {
    if let Some(result) = capture.pending_final.take() {
        return result;
    }
    let recording = &capture.recording;
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        while let Some(event) = recording.try_event() {
            if let NariEvent::Finished(result) = event {
                return result;
            }
        }
        if recording.is_finished() {
            while let Some(event) = recording.try_event() {
                if let NariEvent::Finished(result) = event {
                    return result;
                }
            }
            return Err(VoiceError::CaptureFailed);
        }
        if Instant::now() >= deadline {
            return Err(VoiceError::Timeout);
        }
        std::thread::sleep(Duration::from_millis(30));
    }
}

fn failure_label(error: &VoiceError) -> String {
    match error {
        VoiceError::NariNotConfigured => "Voice not configured".into(),
        VoiceError::Timeout => "Transcription timed out".into(),
        VoiceError::MicrophoneUnavailable => "No microphone recorder found".into(),
        other => format!("Voice failed: {other}"),
    }
}

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    async_io::block_on(future)
}

/// One desktop notification, updated in place (`notify-send -p` / `-r`).
struct Indicator {
    enabled: bool,
    id: std::sync::Mutex<Option<String>>,
}

impl Indicator {
    fn new(enabled: bool) -> Self {
        Self {
            enabled,
            id: std::sync::Mutex::new(None),
        }
    }

    /// Show or replace the notification. Returns whether it was displayed.
    fn show(&self, text: &str) -> bool {
        if !self.enabled {
            return false;
        }
        let mut id = self.id.lock().unwrap_or_else(|p| p.into_inner());
        let mut cmd = Command::new("notify-send");
        cmd.args(["-a", "Jcode", "-p", "-u", "low", "Jcode voice", text]);
        if let Some(previous) = id.as_deref() {
            cmd.args(["-r", previous]);
        }
        let Ok(output) = cmd.stdin(Stdio::null()).stderr(Stdio::null()).output() else {
            return false;
        };
        if !output.status.success() {
            return false;
        }
        let printed = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !printed.is_empty() {
            *id = Some(printed);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failure_labels_are_short() {
        assert_eq!(
            failure_label(&VoiceError::NariNotConfigured),
            "Voice not configured"
        );
        assert_eq!(
            failure_label(&VoiceError::Timeout),
            "Transcription timed out"
        );
    }
}
