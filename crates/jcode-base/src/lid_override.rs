//! Temporarily keep the machine awake when the lid closes on macOS and Windows.
//!
//! Linux gets lid-close blocking for free: `systemd-inhibit` accepts
//! `handle-lid-switch` as a lock that is released when the helper dies. macOS and
//! Windows have no equivalent per-process lock. On those platforms the only way to
//! stop a lid close from sleeping the machine is to change a persistent system
//! setting and change it back afterwards:
//!
//! * macOS: `pmset -a disablesleep 1`. This needs root, so jcode runs it through
//!   `sudo -n` and never prompts. Users who want this opt in with a sudoers rule
//!   (see [`MACOS_SUDOERS_HINT`]). Without it the override is skipped and a
//!   message is logged once.
//! * Windows: the active power plan's "lid close action" (`LIDACTION`) for both
//!   AC and battery is set to "Do nothing". Standard users can change their
//!   active plan, so no elevation is needed.
//!
//! Because these settings outlive the process, the original values are saved to
//! `~/.jcode/lid_override.json` *before* anything is changed. That file is the
//! crash-recovery journal: if the owning process dies (crash, `kill -9`, power
//! loss) the next jcode process that sees a journal with a dead owner restores
//! the saved values. The daemon checks this on every reconcile tick, so a stale
//! override is repaired within seconds of a daemon starting.

use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};

const JOURNAL_FILE: &str = "lid_override.json";

/// Shown when macOS cannot run `pmset` without a password.
pub const MACOS_SUDOERS_HINT: &str = "To let jcode keep your Mac awake with the lid closed while it works, \
run `sudo visudo -f /etc/sudoers.d/jcode-pmset` and add: \
`%admin ALL=(root) NOPASSWD: /usr/bin/pmset -a disablesleep 0, /usr/bin/pmset -a disablesleep 1`. \
Set `[power].block_lid_close = false` to silence this message.";

/// Original lid-related settings captured before jcode changed them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "platform", rename_all = "snake_case")]
pub enum LidSnapshot {
    Macos {
        sleep_disabled: bool,
    },
    Windows {
        /// Active power scheme GUID, in canonical string form.
        scheme: String,
        ac_lid_action: u32,
        dc_lid_action: u32,
    },
    /// Test-only backend state.
    Fake {
        value: u32,
    },
}

/// Platform operations needed to override and restore the lid action.
pub trait LidBackend {
    /// Capture the current settings so they can be restored later.
    fn snapshot(&self) -> io::Result<LidSnapshot>;
    /// Make a lid close keep the machine running.
    fn block(&self, original: &LidSnapshot) -> io::Result<()>;
    /// Put back the settings from `original`.
    fn restore(&self, original: &LidSnapshot) -> io::Result<()>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Journal {
    owner_pid: u32,
    snapshot: LidSnapshot,
}

/// Crash-safe owner of the persistent lid override.
pub struct LidOverride {
    backend: Box<dyn LidBackend + Send>,
    journal_path: PathBuf,
    pid: u32,
    is_alive: fn(u32) -> bool,
    /// Whether this process currently holds the override.
    engaged: bool,
    /// Set after a failure so we do not retry every tick.
    disabled: bool,
    /// Whether the failure has been logged, so each turn does not repeat it.
    warned: bool,
}

impl LidOverride {
    /// Build the override for this platform, or `None` where it is not needed
    /// (Linux handles the lid via `systemd-inhibit`) or not supported.
    pub fn for_current_platform() -> Option<Self> {
        let backend = platform_backend()?;
        let journal_path = crate::storage::jcode_dir().ok()?.join(JOURNAL_FILE);
        // A sandboxed JCODE_HOME (tests, isolated profiles) must never touch
        // machine-wide power settings.
        if crate::storage::running_with_sandboxed_home() {
            return None;
        }
        Some(Self::new(
            backend,
            journal_path,
            std::process::id(),
            crate::platform::is_process_running,
        ))
    }

    fn new(
        backend: Box<dyn LidBackend + Send>,
        journal_path: PathBuf,
        pid: u32,
        is_alive: fn(u32) -> bool,
    ) -> Self {
        Self {
            backend,
            journal_path,
            pid,
            is_alive,
            engaged: false,
            disabled: false,
            warned: false,
        }
    }

    pub fn is_engaged(&self) -> bool {
        self.engaged
    }

    /// Reconcile the override with the desired state. Cheap when nothing
    /// changes: a single file-existence check.
    pub fn set_active(&mut self, active: bool) {
        if active && !self.disabled {
            if !self.engaged
                && let Err(error) = self.engage()
            {
                self.disabled = true;
                if !self.warned {
                    self.warned = true;
                    crate::logging::warn(&format!(
                        "lid_override: could not block sleep on lid close: {error}"
                    ));
                    if cfg!(target_os = "macos") {
                        crate::logging::warn(MACOS_SUDOERS_HINT);
                    }
                }
            }
        } else {
            if !active {
                // Let a later turn try again, e.g. after the user adds the
                // sudoers rule or toggles the config back on.
                self.disabled = false;
            }
            self.disengage();
            self.recover_stale();
        }
    }

    fn engage(&mut self) -> io::Result<()> {
        match self.read_journal() {
            Some(journal) if journal.owner_pid == self.pid => {
                // Same PID: this process re-exec'd itself (daemon hot reload)
                // while engaged. The journal already holds the true originals.
                self.backend.block(&journal.snapshot)?;
                self.engaged = true;
                return Ok(());
            }
            Some(journal) if (self.is_alive)(journal.owner_pid) => {
                // Another live jcode process owns the override, so the lid is
                // already blocked. Leave it alone.
                return Ok(());
            }
            Some(journal) => {
                self.restore_journal(&journal)?;
            }
            None => {}
        }

        let snapshot = self.backend.snapshot()?;
        // Journal first, so a crash between these steps can always be undone.
        write_journal(
            &self.journal_path,
            &Journal {
                owner_pid: self.pid,
                snapshot: snapshot.clone(),
            },
        )?;
        if let Err(error) = self.backend.block(&snapshot) {
            let _ = self.backend.restore(&snapshot);
            let _ = std::fs::remove_file(&self.journal_path);
            return Err(error);
        }
        self.engaged = true;
        crate::logging::info("lid_override: lid close will not sleep while jcode is working");
        Ok(())
    }

    fn disengage(&mut self) {
        if !self.engaged {
            return;
        }
        self.engaged = false;
        match self.read_journal() {
            Some(journal) if journal.owner_pid == self.pid => {
                if let Err(error) = self.restore_journal(&journal) {
                    crate::logging::warn(&format!(
                        "lid_override: failed to restore lid settings: {error}"
                    ));
                } else {
                    crate::logging::info("lid_override: restored original lid settings");
                }
            }
            _ => {}
        }
    }

    /// Restore settings left behind by a jcode process that died while holding
    /// the override.
    pub fn recover_stale(&mut self) {
        let Some(journal) = self.read_journal() else {
            return;
        };
        if journal.owner_pid == self.pid && self.engaged {
            return;
        }
        if journal.owner_pid != self.pid && (self.is_alive)(journal.owner_pid) {
            return;
        }
        match self.restore_journal(&journal) {
            Ok(()) => crate::logging::info(&format!(
                "lid_override: restored lid settings left by exited process {}",
                journal.owner_pid
            )),
            Err(error) => crate::logging::warn(&format!(
                "lid_override: failed to restore stale lid settings: {error}"
            )),
        }
    }

    fn restore_journal(&self, journal: &Journal) -> io::Result<()> {
        self.backend.restore(&journal.snapshot)?;
        match std::fs::remove_file(&self.journal_path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }

    fn read_journal(&self) -> Option<Journal> {
        let bytes = std::fs::read(&self.journal_path).ok()?;
        match serde_json::from_slice(&bytes) {
            Ok(journal) => Some(journal),
            Err(error) => {
                crate::logging::warn(&format!(
                    "lid_override: ignoring unreadable journal {}: {error}",
                    self.journal_path.display()
                ));
                None
            }
        }
    }
}

impl Drop for LidOverride {
    fn drop(&mut self) {
        self.disengage();
    }
}

/// Restore the lid settings if this process owns the override. For exit paths
/// that call `std::process::exit` and therefore skip destructors.
pub fn release_for_exiting_process() {
    let Some(mut lid) = LidOverride::for_current_platform() else {
        return;
    };
    if lid
        .read_journal()
        .is_some_and(|journal| journal.owner_pid == lid.pid)
    {
        lid.engaged = true;
        lid.disengage();
    }
}

fn write_journal(path: &Path, journal: &Journal) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(journal).map_err(io::Error::other)?;
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

#[cfg(target_os = "macos")]
fn platform_backend() -> Option<Box<dyn LidBackend + Send>> {
    Some(Box::new(macos::MacosPmset))
}

#[cfg(windows)]
fn platform_backend() -> Option<Box<dyn LidBackend + Send>> {
    Some(Box::new(windows::WindowsLidAction))
}

#[cfg(not(any(target_os = "macos", windows)))]
fn platform_backend() -> Option<Box<dyn LidBackend + Send>> {
    None
}

/// Parse the `SleepDisabled` flag out of `pmset -g` output.
#[cfg_attr(not(any(test, target_os = "macos")), allow(dead_code))]
fn parse_pmset_sleep_disabled(output: &str) -> bool {
    output.lines().any(|line| {
        let mut parts = line.split_whitespace();
        parts.next() == Some("SleepDisabled") && parts.next() == Some("1")
    })
}

#[cfg(target_os = "macos")]
mod macos {
    use super::{LidBackend, LidSnapshot};
    use std::io;
    use std::process::{Command, Stdio};

    pub(super) struct MacosPmset;

    fn set_disablesleep(on: bool) -> io::Result<()> {
        let output = Command::new("sudo")
            .args(["-n", "/usr/bin/pmset", "-a", "disablesleep"])
            .arg(if on { "1" } else { "0" })
            .stdin(Stdio::null())
            .output()?;
        if output.status.success() {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "`sudo -n pmset -a disablesleep` failed: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
            ))
        }
    }

    impl LidBackend for MacosPmset {
        fn snapshot(&self) -> io::Result<LidSnapshot> {
            let output = Command::new("/usr/bin/pmset")
                .arg("-g")
                .stdin(Stdio::null())
                .output()?;
            if !output.status.success() {
                return Err(io::Error::other("`pmset -g` failed"));
            }
            Ok(LidSnapshot::Macos {
                sleep_disabled: super::parse_pmset_sleep_disabled(&String::from_utf8_lossy(
                    &output.stdout,
                )),
            })
        }

        fn block(&self, original: &LidSnapshot) -> io::Result<()> {
            match original {
                // Sleep is already disabled by the user: nothing to change.
                LidSnapshot::Macos {
                    sleep_disabled: true,
                } => Ok(()),
                _ => set_disablesleep(true),
            }
        }

        fn restore(&self, original: &LidSnapshot) -> io::Result<()> {
            match original {
                LidSnapshot::Macos { sleep_disabled } => set_disablesleep(*sleep_disabled),
                other => Err(io::Error::other(format!(
                    "journal snapshot {other:?} does not belong to macOS"
                ))),
            }
        }
    }
}

/// Format a GUID in canonical `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx` form.
#[cfg_attr(not(any(test, windows)), allow(dead_code))]
fn format_guid(data1: u32, data2: u16, data3: u16, data4: [u8; 8]) -> String {
    format!(
        "{data1:08x}-{data2:04x}-{data3:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        data4[0], data4[1], data4[2], data4[3], data4[4], data4[5], data4[6], data4[7]
    )
}

/// Parse a canonical GUID string back into its parts.
#[cfg_attr(not(any(test, windows)), allow(dead_code))]
fn parse_guid(text: &str) -> Option<(u32, u16, u16, [u8; 8])> {
    let hex: String = text.chars().filter(|c| *c != '-').collect();
    if hex.len() != 32 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let value = u128::from_str_radix(&hex, 16).ok()?;
    Some((
        (value >> 96) as u32,
        (value >> 80) as u16,
        (value >> 64) as u16,
        (value as u64).to_be_bytes(),
    ))
}

#[cfg(windows)]
mod windows {
    use super::{LidBackend, LidSnapshot};
    use std::io;
    use windows_sys::Win32::Foundation::{ERROR_SUCCESS, LocalFree};
    use windows_sys::Win32::System::Power::{
        PowerGetActiveScheme, PowerReadACValueIndex, PowerReadDCValueIndex, PowerSetActiveScheme,
        PowerWriteACValueIndex, PowerWriteDCValueIndex,
    };
    use windows_sys::Win32::System::SystemServices::GUID_SYSTEM_BUTTON_SUBGROUP;
    use windows_sys::core::GUID;

    /// Power setting "Lid close action" (`LIDACTION`).
    const GUID_LIDACTION: GUID = GUID::from_u128(0x5ca83367_6e45_459f_a27b_476b1d01c936);
    /// `LIDACTION` value meaning "Do nothing".
    const LID_ACTION_DO_NOTHING: u32 = 0;

    pub(super) struct WindowsLidAction;

    fn check(code: u32, what: &str) -> io::Result<()> {
        if code == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(io::Error::other(format!(
                "{what} failed: {}",
                io::Error::from_raw_os_error(code as i32)
            )))
        }
    }

    fn active_scheme() -> io::Result<GUID> {
        let mut ptr: *mut GUID = std::ptr::null_mut();
        // SAFETY: PowerGetActiveScheme allocates the GUID with LocalAlloc; we
        // copy it out and free it exactly once.
        unsafe {
            check(
                PowerGetActiveScheme(std::ptr::null_mut(), &mut ptr),
                "PowerGetActiveScheme",
            )?;
            if ptr.is_null() {
                return Err(io::Error::other("PowerGetActiveScheme returned null"));
            }
            let scheme = *ptr;
            LocalFree(ptr.cast());
            Ok(scheme)
        }
    }

    fn scheme_from_snapshot(scheme: &str) -> io::Result<GUID> {
        let (data1, data2, data3, data4) = super::parse_guid(scheme)
            .ok_or_else(|| io::Error::other(format!("invalid scheme GUID {scheme:?}")))?;
        Ok(GUID {
            data1,
            data2,
            data3,
            data4,
        })
    }

    fn write_lid_action(scheme: &GUID, ac: u32, dc: u32) -> io::Result<()> {
        // SAFETY: all pointers reference live stack/static GUIDs.
        unsafe {
            check(
                PowerWriteACValueIndex(
                    std::ptr::null_mut(),
                    scheme,
                    &GUID_SYSTEM_BUTTON_SUBGROUP,
                    &GUID_LIDACTION,
                    ac,
                ),
                "PowerWriteACValueIndex",
            )?;
            check(
                PowerWriteDCValueIndex(
                    std::ptr::null_mut(),
                    scheme,
                    &GUID_SYSTEM_BUTTON_SUBGROUP,
                    &GUID_LIDACTION,
                    dc,
                ),
                "PowerWriteDCValueIndex",
            )?;
            // Re-applying the scheme makes the new values take effect now. Only
            // do that when it is still the active plan, so restoring never
            // switches away from a plan the user picked in the meantime.
            let is_active = active_scheme().is_ok_and(|active| {
                (active.data1, active.data2, active.data3, active.data4)
                    == (scheme.data1, scheme.data2, scheme.data3, scheme.data4)
            });
            if !is_active {
                return Ok(());
            }
            check(
                PowerSetActiveScheme(std::ptr::null_mut(), scheme),
                "PowerSetActiveScheme",
            )
        }
    }

    impl LidBackend for WindowsLidAction {
        fn snapshot(&self) -> io::Result<LidSnapshot> {
            let scheme = active_scheme()?;
            let mut ac = 0u32;
            let mut dc = 0u32;
            // SAFETY: out-pointers reference live locals.
            unsafe {
                check(
                    PowerReadACValueIndex(
                        std::ptr::null_mut(),
                        &scheme,
                        &GUID_SYSTEM_BUTTON_SUBGROUP,
                        &GUID_LIDACTION,
                        &mut ac,
                    ),
                    "PowerReadACValueIndex",
                )?;
                check(
                    PowerReadDCValueIndex(
                        std::ptr::null_mut(),
                        &scheme,
                        &GUID_SYSTEM_BUTTON_SUBGROUP,
                        &GUID_LIDACTION,
                        &mut dc,
                    ),
                    "PowerReadDCValueIndex",
                )?;
            }
            Ok(LidSnapshot::Windows {
                scheme: super::format_guid(scheme.data1, scheme.data2, scheme.data3, scheme.data4),
                ac_lid_action: ac,
                dc_lid_action: dc,
            })
        }

        fn block(&self, original: &LidSnapshot) -> io::Result<()> {
            match original {
                LidSnapshot::Windows { scheme, .. } => write_lid_action(
                    &scheme_from_snapshot(scheme)?,
                    LID_ACTION_DO_NOTHING,
                    LID_ACTION_DO_NOTHING,
                ),
                other => Err(io::Error::other(format!(
                    "journal snapshot {other:?} does not belong to Windows"
                ))),
            }
        }

        fn restore(&self, original: &LidSnapshot) -> io::Result<()> {
            match original {
                LidSnapshot::Windows {
                    scheme,
                    ac_lid_action,
                    dc_lid_action,
                } => write_lid_action(
                    &scheme_from_snapshot(scheme)?,
                    *ac_lid_action,
                    *dc_lid_action,
                ),
                other => Err(io::Error::other(format!(
                    "journal snapshot {other:?} does not belong to Windows"
                ))),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// In-memory stand-in for the OS setting.
    #[derive(Clone)]
    struct Fake {
        value: Arc<Mutex<u32>>,
        fail_block: bool,
    }

    impl LidBackend for Fake {
        fn snapshot(&self) -> io::Result<LidSnapshot> {
            Ok(LidSnapshot::Fake {
                value: *self.value.lock().unwrap(),
            })
        }
        fn block(&self, _: &LidSnapshot) -> io::Result<()> {
            if self.fail_block {
                return Err(io::Error::new(io::ErrorKind::PermissionDenied, "denied"));
            }
            *self.value.lock().unwrap() = 0;
            Ok(())
        }
        fn restore(&self, original: &LidSnapshot) -> io::Result<()> {
            let LidSnapshot::Fake { value } = original else {
                panic!("wrong snapshot");
            };
            *self.value.lock().unwrap() = *value;
            Ok(())
        }
    }

    const USER_SETTING: u32 = 1; // e.g. "Sleep"

    fn fake(fail_block: bool) -> (Fake, Arc<Mutex<u32>>) {
        let value = Arc::new(Mutex::new(USER_SETTING));
        (
            Fake {
                value: Arc::clone(&value),
                fail_block,
            },
            value,
        )
    }

    fn alive(pid: u32) -> bool {
        pid < 1000
    }

    fn make(backend: Fake, path: &Path, pid: u32) -> LidOverride {
        LidOverride::new(Box::new(backend), path.to_path_buf(), pid, alive)
    }

    #[test]
    fn engages_then_restores_original_setting() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(JOURNAL_FILE);
        let (backend, value) = fake(false);
        let mut lid = make(backend, &path, 10);

        lid.set_active(true);
        assert!(lid.is_engaged());
        assert_eq!(*value.lock().unwrap(), 0);
        assert!(path.exists(), "journal must exist while engaged");

        lid.set_active(false);
        assert!(!lid.is_engaged());
        assert_eq!(*value.lock().unwrap(), USER_SETTING);
        assert!(!path.exists());
    }

    #[test]
    fn drop_restores_setting() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(JOURNAL_FILE);
        let (backend, value) = fake(false);
        {
            let mut lid = make(backend, &path, 10);
            lid.set_active(true);
        }
        assert_eq!(*value.lock().unwrap(), USER_SETTING);
        assert!(!path.exists());
    }

    #[test]
    fn crashed_owner_is_recovered_by_next_process() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(JOURNAL_FILE);
        let (backend, value) = fake(false);

        let mut crashed = make(backend.clone(), &path, 5000); // pid 5000 is "dead"
        crashed.set_active(true);
        std::mem::forget(crashed); // simulate kill -9: no Drop, no restore
        assert_eq!(*value.lock().unwrap(), 0);

        // An idle daemon tick repairs it.
        let mut next = make(backend, &path, 20);
        next.set_active(false);
        assert_eq!(*value.lock().unwrap(), USER_SETTING);
        assert!(!path.exists());
    }

    #[test]
    fn crashed_owner_originals_are_kept_when_next_process_engages() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(JOURNAL_FILE);
        let (backend, value) = fake(false);

        let mut crashed = make(backend.clone(), &path, 5000);
        crashed.set_active(true);
        std::mem::forget(crashed);

        let mut next = make(backend, &path, 20);
        next.set_active(true);
        assert!(next.is_engaged());
        next.set_active(false);
        // Must be the user's value, not the "blocked" value the crash left.
        assert_eq!(*value.lock().unwrap(), USER_SETTING);
    }

    #[test]
    fn live_foreign_owner_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(JOURNAL_FILE);
        let (backend, value) = fake(false);

        let mut owner = make(backend.clone(), &path, 10);
        owner.set_active(true);

        let mut other = make(backend, &path, 11);
        other.set_active(true);
        assert!(!other.is_engaged());
        other.set_active(false);
        assert_eq!(
            *value.lock().unwrap(),
            0,
            "other must not undo owner's override"
        );
        assert!(path.exists());

        owner.set_active(false);
        assert_eq!(*value.lock().unwrap(), USER_SETTING);
    }

    #[test]
    fn reexec_with_same_pid_reuses_saved_originals() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(JOURNAL_FILE);
        let (backend, value) = fake(false);

        let mut before = make(backend.clone(), &path, 10);
        before.set_active(true);
        std::mem::forget(before); // execv replaces the image without Drop

        let mut after = make(backend, &path, 10);
        after.set_active(true);
        assert!(after.is_engaged());
        after.set_active(false);
        assert_eq!(*value.lock().unwrap(), USER_SETTING);
        assert!(!path.exists());
    }

    #[test]
    fn failed_block_rolls_back_and_stops_retrying_until_idle() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(JOURNAL_FILE);
        let (backend, value) = fake(true);
        let mut lid = make(backend, &path, 10);

        lid.set_active(true);
        assert!(!lid.is_engaged());
        assert!(lid.disabled);
        assert!(!path.exists());
        assert_eq!(*value.lock().unwrap(), USER_SETTING);

        lid.set_active(false);
        assert!(!lid.disabled, "idle resets the retry latch");
    }

    #[test]
    fn pmset_output_parsing() {
        let on = "System-wide power settings:\n SleepDisabled\t\t1\nCurrently in use:\n sleep 1\n";
        let off = "System-wide power settings:\n SleepDisabled\t\t0\n";
        assert!(parse_pmset_sleep_disabled(on));
        assert!(!parse_pmset_sleep_disabled(off));
        assert!(!parse_pmset_sleep_disabled("Currently in use:\n sleep 1\n"));
    }

    #[test]
    fn guid_round_trips() {
        let text = "381b4222-f694-41f0-9685-ff5bb260df2e"; // Balanced plan
        let (d1, d2, d3, d4) = parse_guid(text).unwrap();
        assert_eq!(d1, 0x381b4222);
        assert_eq!(d2, 0xf694);
        assert_eq!(d3, 0x41f0);
        assert_eq!(format_guid(d1, d2, d3, d4), text);
        assert!(parse_guid("nope").is_none());
    }

    #[test]
    fn snapshot_journal_format_is_stable() {
        let journal = Journal {
            owner_pid: 42,
            snapshot: LidSnapshot::Windows {
                scheme: "381b4222-f694-41f0-9685-ff5bb260df2e".into(),
                ac_lid_action: 1,
                dc_lid_action: 1,
            },
        };
        let json = serde_json::to_string(&journal).unwrap();
        assert!(json.contains("\"platform\":\"windows\""));
        let back: Journal = serde_json::from_str(&json).unwrap();
        assert_eq!(back.snapshot, journal.snapshot);
    }
}
