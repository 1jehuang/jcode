//! Process-wide "keep the machine awake while jcode is working" inhibitor.
//!
//! The shared `jcode serve` daemon hosts every session, so a single inhibitor
//! living in that process is enough to keep the laptop awake while *any* session
//! is streaming/processing (the same signal Waybar surfaces as "N streaming").
//!
//! The helper process is only kept alive while active work exists, then killed
//! immediately so normal power management resumes the moment work finishes.
//!
//! ## Crash / reload safety
//!
//! The daemon reloads itself with `execv` (the PID stays the same but the
//! process image is replaced) and can also be `kill -9`'d. In both cases a
//! child spawned with `sleep infinity` would be orphaned and hold the inhibitor
//! lock forever. To make leaks self-heal, the helper is spawned with a bounded
//! TTL (`sleep <TTL>`) and refreshed periodically while work continues. After a
//! crash or reload the stale lock expires within at most `INHIBIT_TTL`, and the
//! freshly-started process re-acquires on its next reconcile tick.

use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Legacy/global override shared with the desktop app: when set, never inhibit.
const DISABLE_ENV: &str = "JCODE_DISABLE_POWER_INHIBIT";

/// How long each spawned helper holds the lock before it must be refreshed.
/// Bounding this is what makes orphaned locks self-heal after a crash/reload.
const INHIBIT_TTL: Duration = Duration::from_secs(150);

/// Refresh once the current helper has been held longer than this. Kept well
/// below `INHIBIT_TTL` so coverage never lapses between reconcile ticks.
const INHIBIT_REFRESH_AFTER: Duration = Duration::from_secs(90);

/// Best-effort inhibitor that keeps the machine awake while jcode is actively
/// streaming/processing.
pub struct PowerInhibitor {
    child: Option<Child>,
    acquired_at: Option<Instant>,
    available: bool,
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
        Self {
            child: None,
            acquired_at: None,
            available: power_inhibit_available(
                std::env::var_os(DISABLE_ENV).is_some(),
                current_platform(),
            ),
        }
    }

    /// Whether this inhibitor can actually do anything on this platform/env.
    pub fn is_available(&self) -> bool {
        self.available
    }

    /// Reconcile the helper process against the desired active state. Safe to
    /// call frequently; it is idempotent and also refreshes the bounded TTL.
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
        let healthy = self.child.as_mut().is_some_and(child_is_running);
        let fresh = self
            .acquired_at
            .is_some_and(|at| !should_refresh(at, now, INHIBIT_REFRESH_AFTER));
        if healthy && fresh {
            return;
        }

        // Either there is no helper, it exited, or its TTL is close to expiring:
        // (re)spawn a fresh one and drop the old.
        self.release();

        let Some(platform) = current_platform() else {
            self.available = false;
            return;
        };

        match build_inhibit_command(platform, INHIBIT_TTL).spawn() {
            Ok(child) => {
                self.child = Some(child);
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
        if let Some(mut child) = self.child.take() {
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
    }
}

impl Drop for PowerInhibitor {
    fn drop(&mut self) {
        self.release();
    }
}

fn child_is_running(child: &mut Child) -> bool {
    matches!(child.try_wait(), Ok(None))
}

/// Whether a helper acquired at `acquired_at` should be refreshed by `now`.
fn should_refresh(acquired_at: Instant, now: Instant, refresh_after: Duration) -> bool {
    now.saturating_duration_since(acquired_at) >= refresh_after
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InhibitPlatform {
    LinuxSystemd,
    MacosCaffeinate,
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
    } else {
        None
    }
}

fn build_inhibit_command(platform: InhibitPlatform, ttl: Duration) -> Command {
    match platform {
        InhibitPlatform::LinuxSystemd => build_linux_systemd_inhibit_command(ttl),
        InhibitPlatform::MacosCaffeinate => build_macos_caffeinate_command(ttl),
    }
}

fn build_linux_systemd_inhibit_command(ttl: Duration) -> Command {
    let mut command = Command::new("systemd-inhibit");
    command
        .arg("--what=sleep:handle-lid-switch")
        .arg("--who=jcode")
        .arg("--why=Jcode is streaming or processing active work")
        .arg("--mode=block")
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
    }

    #[test]
    fn linux_inhibitor_blocks_sleep_and_lid_switch_with_bounded_ttl() {
        let command = super::build_inhibit_command(InhibitPlatform::LinuxSystemd, INHIBIT_TTL);
        let args = command_args(&command);

        assert_eq!(command_name(&command), "systemd-inhibit");
        assert!(args.contains(&"--what=sleep:handle-lid-switch".to_string()));
        assert!(args.contains(&"--who=jcode".to_string()));
        assert!(args.contains(&"--mode=block".to_string()));
        assert!(args.contains(&"sleep".to_string()));
        // Bounded TTL (not "infinity") so orphaned locks self-heal.
        assert!(args.contains(&INHIBIT_TTL.as_secs().to_string()));
        assert!(!args.contains(&"infinity".to_string()));
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
}
