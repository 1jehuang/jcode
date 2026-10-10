//! Process-wide "keep the machine awake while jcode is working" inhibitor.
//!
//! The shared `jcode serve` daemon hosts every session, so a single inhibitor
//! living in that process is enough to keep the laptop awake while *any* session
//! is streaming/processing (the same signal Waybar surfaces as "N streaming").
//!
//! The platform guard is only kept alive while active work exists, then released
//! immediately so normal power management resumes the moment work finishes.
//!
//! ## Crash / reload safety
//!
//! The daemon reloads itself with `execv` (the PID stays the same but the
//! process image is replaced) and can also be `kill -9`'d. In both cases a
//! child spawned with `sleep infinity` would be orphaned and hold the inhibitor
//! lock forever. To make Unix helper leaks self-heal, each helper is spawned with
//! a bounded TTL (`sleep <TTL>`) and refreshed periodically while work continues.
//! Windows uses a dedicated in-process thread instead; Windows automatically
//! clears that thread's execution-state request if the process exits or crashes.

use std::io;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

#[cfg(windows)]
use std::sync::mpsc::{self, Receiver, Sender};
#[cfg(windows)]
use std::thread::{self, JoinHandle};

/// Legacy/global override shared with the desktop app: when set, never inhibit.
const DISABLE_ENV: &str = "JCODE_DISABLE_POWER_INHIBIT";

/// How long each spawned helper holds the lock before it must be refreshed.
/// Bounding this is what makes orphaned locks self-heal after a crash/reload.
const INHIBIT_TTL: Duration = Duration::from_secs(150);

/// Refresh once the current helper has been held longer than this. Kept well
/// below `INHIBIT_TTL` so coverage never lapses between reconcile ticks.
const INHIBIT_REFRESH_AFTER: Duration = Duration::from_secs(90);

/// How long a replaced helper is kept alive after its successor is spawned.
/// logind re-evaluates a closed lid on every event, so even a few milliseconds
/// without a registered lock suspends the machine. The new helper needs time to
/// register over D-Bus before the old one may go.
const HANDOVER_OVERLAP: Duration = Duration::from_secs(3);

/// First retry delay after the systemd helper fails (for example because logind
/// refuses new locks while a suspend/resume is still in progress). Doubles on
/// each consecutive failure up to [`MAX_RETRY_BACKOFF`].
const INITIAL_RETRY_BACKOFF: Duration = Duration::from_secs(10);
const MAX_RETRY_BACKOFF: Duration = Duration::from_secs(600);

/// Best-effort inhibitor that keeps the machine awake while jcode is actively
/// streaming/processing.
pub struct PowerInhibitor {
    handle: Option<InhibitHandle>,
    acquired_at: Option<Instant>,
    /// Previous helper kept alive until its replacement has had time to register.
    retiring: Option<InhibitHandle>,
    platform: Option<InhibitPlatform>,
    available: bool,
    /// After a helper failure, do not respawn before this instant.
    retry_at: Option<Instant>,
    retry_backoff: Duration,
}

enum InhibitHandle {
    Child(Child),
    #[cfg(windows)]
    Windows(WindowsPowerGuard),
}

struct InhibitHandleStatus {
    running: bool,
    exit_status: Option<ExitStatus>,
}

impl InhibitHandle {
    fn status(&mut self) -> InhibitHandleStatus {
        match self {
            Self::Child(child) => child_status(child),
            #[cfg(windows)]
            Self::Windows(guard) => InhibitHandleStatus {
                running: guard.is_running(),
                exit_status: None,
            },
        }
    }
}

impl Default for PowerInhibitor {
    fn default() -> Self {
        Self::new()
    }
}

impl PowerInhibitor {
    /// Build an inhibitor. The inhibitor is "available" on supported platforms
    /// unless the legacy `JCODE_DISABLE_POWER_INHIBIT` env escape hatch is set.
    ///
    /// The user-facing config toggle is intentionally *not* baked in here: the
    /// caller evaluates it per-reconcile (via [`PowerInhibitor::set_active`]) so
    /// it can be flipped at runtime in either direction without a restart.
    pub fn new() -> Self {
        let platform = current_platform();
        Self {
            handle: None,
            acquired_at: None,
            retiring: None,
            platform,
            available: power_inhibit_available(std::env::var_os(DISABLE_ENV).is_some(), platform),
            retry_at: None,
            retry_backoff: INITIAL_RETRY_BACKOFF,
        }
    }

    /// Whether this inhibitor can actually do anything on this platform/env.
    pub fn is_available(&self) -> bool {
        self.available
    }

    /// Reconcile the platform guard against the desired active state. Safe to
    /// call frequently; it is idempotent and refreshes bounded Unix helpers.
    pub fn set_active(&mut self, active: bool) {
        if !self.available {
            return;
        }

        if active {
            self.acquire();
        } else {
            self.release();
        }
    }

    fn acquire(&mut self) {
        let now = Instant::now();
        let handle_status = self.handle.as_mut().map(InhibitHandle::status);
        if self.platform == Some(InhibitPlatform::LinuxSystemd)
            && handle_status
                .as_ref()
                .and_then(|status| status.exit_status.as_ref())
                .is_some_and(|status| !status.success())
        {
            // Usually transient: logind rejects new locks while a suspend or
            // resume is in progress. Back off and retry rather than giving up
            // for the rest of the process lifetime.
            crate::logging::warn(&format!(
                "power_inhibit: systemd inhibitor exited unsuccessfully; retrying in {}s",
                self.retry_backoff.as_secs()
            ));
            self.release();
            self.retry_at = Some(now + self.retry_backoff);
            self.retry_backoff = (self.retry_backoff * 2).min(MAX_RETRY_BACKOFF);
            return;
        }
        if self.retry_at.is_some_and(|at| now < at) {
            return;
        }

        let healthy = handle_status.is_some_and(|status| status.running);
        if healthy
            && self
                .acquired_at
                .is_some_and(|at| now.saturating_duration_since(at) >= HANDOVER_OVERLAP)
        {
            // The replacement has had time to register: a lock that survived
            // this long is working, so reset the failure backoff too.
            self.retire_previous();
            self.retry_backoff = INITIAL_RETRY_BACKOFF;
        }
        let fresh = self.platform.is_some_and(|platform| {
            !platform.requires_refresh()
                || self
                    .acquired_at
                    .is_some_and(|at| !should_refresh(at, now, INHIBIT_REFRESH_AFTER))
        });
        if healthy && fresh {
            return;
        }

        // Either there is no helper, it exited, or its TTL is close to expiring.
        // Make before break: keep a still-running helper alive until the new one
        // has registered, otherwise a closed lid suspends in the gap.
        self.retire_previous();
        self.acquired_at = None;
        if let Some(handle) = self.handle.take() {
            if healthy {
                self.retiring = Some(handle);
            } else {
                stop_handle(handle);
            }
        }

        let Some(platform) = self.platform else {
            self.available = false;
            return;
        };

        let acquired = match platform {
            InhibitPlatform::LinuxSystemd | InhibitPlatform::MacosCaffeinate => {
                build_inhibit_command(platform, INHIBIT_TTL)
                    .spawn()
                    .map(InhibitHandle::Child)
            }
            InhibitPlatform::WindowsExecutionState => acquire_windows_inhibit_handle(),
        };

        match acquired {
            Ok(handle) => {
                self.handle = Some(handle);
                self.acquired_at = Some(now);
            }
            Err(error) => {
                crate::logging::warn(&format!(
                    "power_inhibit: failed to acquire inhibitor: {error}"
                ));
                self.available = false;
            }
        }
    }

    fn release(&mut self) {
        self.acquired_at = None;
        self.retire_previous();
        if let Some(handle) = self.handle.take() {
            stop_handle(handle);
        }
    }

    fn retire_previous(&mut self) {
        if let Some(handle) = self.retiring.take() {
            stop_handle(handle);
        }
    }
}

fn stop_handle(handle: InhibitHandle) {
    match handle {
        InhibitHandle::Child(mut child) => {
            if let Err(error) = child.kill() {
                crate::logging::warn(&format!(
                    "power_inhibit: failed to stop inhibitor process: {error}"
                ));
            }
            if let Err(error) = child.wait() {
                crate::logging::warn(&format!(
                    "power_inhibit: failed to reap inhibitor process: {error}"
                ));
            }
        }
        #[cfg(windows)]
        InhibitHandle::Windows(mut guard) => {
            if let Err(error) = guard.stop() {
                crate::logging::warn(&format!(
                    "power_inhibit: failed to release Windows execution state: {error}"
                ));
            }
        }
    }
}

impl Drop for PowerInhibitor {
    fn drop(&mut self) {
        self.release();
    }
}

fn child_status(child: &mut Child) -> InhibitHandleStatus {
    match child.try_wait() {
        Ok(None) => InhibitHandleStatus {
            running: true,
            exit_status: None,
        },
        Ok(Some(exit_status)) => InhibitHandleStatus {
            running: false,
            exit_status: Some(exit_status),
        },
        Err(_) => InhibitHandleStatus {
            running: false,
            exit_status: None,
        },
    }
}

/// Whether a helper acquired at `acquired_at` should be refreshed by `now`.
fn should_refresh(acquired_at: Instant, now: Instant, refresh_after: Duration) -> bool {
    now.saturating_duration_since(acquired_at) >= refresh_after
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InhibitPlatform {
    LinuxSystemd,
    MacosCaffeinate,
    WindowsExecutionState,
}

impl InhibitPlatform {
    fn requires_refresh(self) -> bool {
        !matches!(self, Self::WindowsExecutionState)
    }
}

fn power_inhibit_available(
    legacy_disable_present: bool,
    platform: Option<InhibitPlatform>,
) -> bool {
    !legacy_disable_present && platform.is_some()
}

fn current_platform() -> Option<InhibitPlatform> {
    if cfg!(target_os = "linux") {
        Some(InhibitPlatform::LinuxSystemd)
    } else if cfg!(target_os = "macos") {
        Some(InhibitPlatform::MacosCaffeinate)
    } else if cfg!(windows) {
        Some(InhibitPlatform::WindowsExecutionState)
    } else {
        None
    }
}

fn build_inhibit_command(platform: InhibitPlatform, ttl: Duration) -> Command {
    match platform {
        InhibitPlatform::LinuxSystemd => build_linux_systemd_inhibit_command(ttl),
        InhibitPlatform::MacosCaffeinate => build_macos_caffeinate_command(ttl),
        InhibitPlatform::WindowsExecutionState => {
            unreachable!("Windows uses an in-process execution-state guard")
        }
    }
}

#[cfg(windows)]
fn acquire_windows_inhibit_handle() -> io::Result<InhibitHandle> {
    WindowsPowerGuard::acquire().map(InhibitHandle::Windows)
}

#[cfg(not(windows))]
fn acquire_windows_inhibit_handle() -> io::Result<InhibitHandle> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Windows execution-state guard requested on a non-Windows platform",
    ))
}

#[cfg(windows)]
struct WindowsPowerGuard {
    stop_tx: Option<Sender<()>>,
    done_rx: Option<Receiver<io::Result<()>>>,
    thread: Option<JoinHandle<()>>,
}

#[cfg(windows)]
impl WindowsPowerGuard {
    fn acquire() -> io::Result<Self> {
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let (stop_tx, stop_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name("jcode-power-inhibit".to_string())
            .spawn(move || run_windows_power_guard(ready_tx, stop_rx, done_tx))?;

        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                stop_tx: Some(stop_tx),
                done_rx: Some(done_rx),
                thread: Some(thread),
            }),
            Ok(Err(error)) => {
                if thread.join().is_err() {
                    return Err(io::Error::other("Windows power guard thread panicked"));
                }
                Err(error)
            }
            Err(error) => {
                if thread.join().is_err() {
                    return Err(io::Error::other("Windows power guard thread panicked"));
                }
                Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    format!("Windows power guard exited before acquisition: {error}"),
                ))
            }
        }
    }

    fn is_running(&self) -> bool {
        self.thread
            .as_ref()
            .is_some_and(|thread| !thread.is_finished())
    }

    fn stop(&mut self) -> io::Result<()> {
        if let Some(stop_tx) = self.stop_tx.take() {
            let _stop_signal_sent = stop_tx.send(()).is_ok();
        }

        let clear_result = match self.done_rx.take() {
            Some(done_rx) => done_rx.recv().map_err(|error| {
                io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    format!("Windows power guard exited before cleanup: {error}"),
                )
            })?,
            None => Ok(()),
        };

        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            return Err(io::Error::other("Windows power guard thread panicked"));
        }

        clear_result
    }
}

#[cfg(windows)]
impl Drop for WindowsPowerGuard {
    fn drop(&mut self) {
        if let Err(error) = self.stop() {
            jcode_logging::warn(&format!("failed to stop Windows power guard: {error}"));
        }
    }
}

#[cfg(windows)]
fn run_windows_power_guard(
    ready_tx: mpsc::SyncSender<io::Result<()>>,
    stop_rx: Receiver<()>,
    done_tx: mpsc::SyncSender<io::Result<()>>,
) {
    use windows_sys::Win32::System::Power::SetThreadExecutionState;

    let acquired = unsafe { SetThreadExecutionState(windows_execution_state_flags()) };
    if acquired == 0 {
        drop(ready_tx.send(Err(io::Error::last_os_error())));
        return;
    }

    if ready_tx.send(Ok(())).is_err() {
        if unsafe { SetThreadExecutionState(windows_clear_execution_state_flags()) } == 0 {
            jcode_logging::warn("failed to clear Windows power guard after receiver disconnect");
        }
        return;
    }

    let _stop_requested = stop_rx.recv().is_ok();
    let cleared = unsafe { SetThreadExecutionState(windows_clear_execution_state_flags()) };
    let result = if cleared == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    };
    drop(done_tx.send(result));
}

#[cfg(windows)]
fn windows_execution_state_flags() -> u32 {
    use windows_sys::Win32::System::Power::{ES_CONTINUOUS, ES_SYSTEM_REQUIRED};

    ES_CONTINUOUS | ES_SYSTEM_REQUIRED
}

#[cfg(windows)]
fn windows_clear_execution_state_flags() -> u32 {
    windows_sys::Win32::System::Power::ES_CONTINUOUS
}

fn build_linux_systemd_inhibit_command(ttl: Duration) -> Command {
    let mut command = Command::new("systemd-inhibit");
    command
        .arg("--what=sleep:handle-lid-switch")
        .arg("--who=jcode")
        .arg("--why=Jcode is streaming or processing active work")
        .arg("--mode=block")
        // Power inhibition is best-effort and must never trigger an interactive
        // Polkit authentication prompt from the background daemon.
        .arg("--no-ask-password")
        .arg("sleep")
        .arg(ttl.as_secs().to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

fn build_macos_caffeinate_command(ttl: Duration) -> Command {
    let mut command = Command::new("caffeinate");
    command
        // -i prevents idle sleep. -s prevents system sleep while on AC power.
        // We intentionally do not use -d so the display can still sleep/turn off.
        // -t bounds the assertion so a crashed/reloaded daemon self-heals.
        .arg("-i")
        .arg("-s")
        .arg("-t")
        .arg(ttl.as_secs().to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

#[cfg(test)]
mod tests {
    use super::{INHIBIT_TTL, InhibitPlatform, should_refresh};
    use std::time::{Duration, Instant};

    fn command_name(command: &std::process::Command) -> String {
        command.get_program().to_string_lossy().to_string()
    }

    fn command_args(command: &std::process::Command) -> Vec<String> {
        command
            .get_args()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect::<Vec<_>>()
    }

    #[test]
    fn availability_requires_no_legacy_disable_and_supported_platform() {
        assert!(super::power_inhibit_available(
            false,
            Some(InhibitPlatform::LinuxSystemd),
        ));
        // Legacy env escape hatch wins.
        assert!(!super::power_inhibit_available(
            true,
            Some(InhibitPlatform::LinuxSystemd),
        ));
        // Unsupported platform.
        assert!(!super::power_inhibit_available(false, None));
        assert!(super::power_inhibit_available(
            false,
            Some(InhibitPlatform::WindowsExecutionState),
        ));
    }

    #[test]
    fn linux_inhibitor_blocks_sleep_and_lid_switch_with_bounded_ttl() {
        let command = super::build_inhibit_command(InhibitPlatform::LinuxSystemd, INHIBIT_TTL);
        let args = command_args(&command);

        assert_eq!(command_name(&command), "systemd-inhibit");
        assert!(args.contains(&"--what=sleep:handle-lid-switch".to_string()));
        assert!(args.contains(&"--who=jcode".to_string()));
        assert!(args.contains(&"--mode=block".to_string()));
        assert!(args.contains(&"--no-ask-password".to_string()));
        assert!(args.contains(&"sleep".to_string()));
        // Bounded TTL (not "infinity") so orphaned locks self-heal.
        assert!(args.contains(&INHIBIT_TTL.as_secs().to_string()));
        assert!(!args.contains(&"infinity".to_string()));
    }

    #[cfg(unix)]
    #[test]
    fn failed_linux_inhibitor_backs_off_instead_of_disabling_forever() {
        let mut child = std::process::Command::new("sh")
            .args(["-c", "exit 1"])
            .spawn()
            .expect("spawn failed inhibitor helper");
        assert!(!child.wait().expect("wait for failed helper").success());

        let mut inhibitor = super::PowerInhibitor {
            handle: Some(super::InhibitHandle::Child(child)),
            acquired_at: Some(Instant::now()),
            retiring: None,
            platform: Some(InhibitPlatform::LinuxSystemd),
            available: true,
            retry_at: None,
            retry_backoff: super::INITIAL_RETRY_BACKOFF,
        };

        inhibitor.set_active(true);

        // A failure after suspend/resume must not kill lid blocking for the
        // rest of the daemon's life.
        assert!(inhibitor.is_available());
        assert!(inhibitor.handle.is_none());
        assert!(inhibitor.retry_at.is_some());
        assert_eq!(inhibitor.retry_backoff, super::INITIAL_RETRY_BACKOFF * 2);

        // Still inside the backoff window: no respawn yet.
        inhibitor.set_active(true);
        assert!(inhibitor.handle.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn refresh_keeps_previous_helper_alive_until_successor_registers() {
        let old = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn stand-in helper");
        let old_pid = old.id();
        let mut inhibitor = super::PowerInhibitor {
            handle: Some(super::InhibitHandle::Child(old)),
            // Old enough that a refresh is due.
            acquired_at: Some(Instant::now() - super::INHIBIT_REFRESH_AFTER),
            retiring: None,
            // Mac platform so the respawn uses a harmless command in tests on
            // Linux (`caffeinate` is missing there, which simply fails spawn).
            platform: Some(InhibitPlatform::MacosCaffeinate),
            available: true,
            retry_at: None,
            retry_backoff: super::INITIAL_RETRY_BACKOFF,
        };

        inhibitor.set_active(true);

        // Whatever happened to the successor, the old lock must not have been
        // dropped first if the successor spawned. If spawn failed (no
        // caffeinate), the old helper is still parked as retiring until release.
        let retiring_pid = match inhibitor.retiring.as_ref() {
            Some(super::InhibitHandle::Child(child)) => Some(child.id()),
            _ => None,
        };
        assert_eq!(retiring_pid, Some(old_pid));
        inhibitor.release();
        assert!(inhibitor.retiring.is_none());
    }

    #[test]
    fn macos_inhibitor_prevents_system_sleep_without_display_assertion() {
        let command = super::build_inhibit_command(InhibitPlatform::MacosCaffeinate, INHIBIT_TTL);
        let args = command_args(&command);

        assert_eq!(command_name(&command), "caffeinate");
        assert!(args.contains(&"-i".to_string()));
        assert!(args.contains(&"-s".to_string()));
        assert!(!args.contains(&"-d".to_string()));
        assert!(args.contains(&"-t".to_string()));
        assert!(args.contains(&INHIBIT_TTL.as_secs().to_string()));
    }

    #[test]
    fn refresh_is_due_only_after_the_threshold_elapses() {
        let acquired = Instant::now();
        let refresh_after = Duration::from_secs(90);
        assert!(!should_refresh(
            acquired,
            acquired + Duration::from_secs(30),
            refresh_after
        ));
        assert!(!should_refresh(
            acquired,
            acquired + Duration::from_secs(89),
            refresh_after
        ));
        assert!(should_refresh(
            acquired,
            acquired + Duration::from_secs(90),
            refresh_after
        ));
        assert!(should_refresh(
            acquired,
            acquired + Duration::from_secs(120),
            refresh_after
        ));
    }

    #[test]
    fn windows_guard_is_long_lived_instead_of_ttl_refreshed() {
        assert!(!InhibitPlatform::WindowsExecutionState.requires_refresh());
        assert!(InhibitPlatform::LinuxSystemd.requires_refresh());
        assert!(InhibitPlatform::MacosCaffeinate.requires_refresh());
    }

    #[cfg(windows)]
    #[test]
    fn windows_guard_prevents_idle_system_sleep_and_releases_cleanly() {
        use windows_sys::Win32::System::Power::{
            ES_AWAYMODE_REQUIRED, ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED,
        };

        let flags = super::windows_execution_state_flags();
        assert_eq!(flags, ES_CONTINUOUS | ES_SYSTEM_REQUIRED);
        assert_eq!(flags & ES_DISPLAY_REQUIRED, 0);
        assert_eq!(flags & ES_AWAYMODE_REQUIRED, 0);

        let mut guard = super::WindowsPowerGuard::acquire().expect("acquire Windows power guard");
        assert!(guard.is_running());
        guard.stop().expect("release Windows power guard");
        assert!(!guard.is_running());
    }
}
