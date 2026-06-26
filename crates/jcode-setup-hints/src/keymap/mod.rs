//! Keymap discovery: snapshot the key bindings that exist on the machine
//! (macOS system shortcuts + terminal emulator bindings) so jcode can detect
//! when one of them intercepts a key jcode wants to use.
//!
//! This module is the data layer. It:
//!   1. discovers bindings from each source ([`macos_hotkeys`], [`terminal`]),
//!   2. normalizes them to [`KeyChord`]s ([`chord`]),
//!   3. records them in a durable JSON snapshot ([`KeymapSnapshot`]).
//!
//! Conflict detection against jcode's own bindings is built on top of this in a
//! later layer.

pub mod chord;
pub mod conflicts;
pub mod macos_hotkeys;
pub mod report;
pub mod source;
pub mod terminal;

pub use chord::KeyChord;
pub use conflicts::{Conflict, JcodeBinding, conflict_signature, detect_conflicts, jcode_bindings};
pub use report::{render_report, render_status_line};
pub use source::{DiscoveredBinding, KeySource};

use serde::{Deserialize, Serialize};

/// Schema version for the on-disk snapshot. Bump when the format changes.
const SNAPSHOT_VERSION: u32 = 1;

/// A durable record of the key bindings discovered on this machine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeymapSnapshot {
    pub version: u32,
    /// RFC3339-ish timestamp of when the snapshot was taken.
    pub captured_at: String,
    pub os: String,
    /// Detected terminal label (e.g. "Ghostty"), best-effort.
    pub terminal: String,
    /// Terminal version string if known.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub terminal_version: String,
    /// All discovered bindings, across every source.
    pub bindings: Vec<DiscoveredBinding>,
}

impl KeymapSnapshot {
    /// Bindings originating from a particular source.
    pub fn from_source(&self, source: KeySource) -> impl Iterator<Item = &DiscoveredBinding> {
        self.bindings.iter().filter(move |b| b.source == source)
    }
}

/// Detect the terminal name in a cross-platform, dependency-light way. On macOS
/// we mirror the detection used elsewhere; on other platforms we fall back to
/// `TERM_PROGRAM`/`TERM`.
fn detect_terminal_label() -> String {
    let term_program = std::env::var("TERM_PROGRAM").unwrap_or_default();
    let term = std::env::var("TERM").unwrap_or_default();
    if std::env::var("GHOSTTY_RESOURCES_DIR").is_ok()
        || term_program.eq_ignore_ascii_case("ghostty")
        || term.to_lowercase().contains("ghostty")
    {
        return "Ghostty".to_string();
    }
    match term_program.to_lowercase().as_str() {
        "iterm.app" => "iTerm2".to_string(),
        "apple_terminal" => "Terminal.app".to_string(),
        "wezterm" => "WezTerm".to_string(),
        "vscode" => "VS Code terminal".to_string(),
        "" => {
            if term.to_lowercase().contains("alacritty") {
                "Alacritty".to_string()
            } else if term.to_lowercase().contains("kitty") {
                "kitty".to_string()
            } else {
                term
            }
        }
        other => other.to_string(),
    }
}

fn now_timestamp() -> String {
    // Avoid pulling in chrono here; seconds since epoch is enough to detect a
    // stale snapshot, and we render it back as a readable value at display time.
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_default()
}

/// Collect all discovered bindings on the current machine. This shells out to
/// the platform tools (`defaults`/`plutil`, `ghostty +list-keybinds`) and so
/// should be called off the hot path / at startup, not per-frame.
pub fn collect_snapshot() -> KeymapSnapshot {
    let mut bindings = Vec::new();
    bindings.extend(macos_hotkeys::read_symbolic_hotkeys());
    bindings.extend(terminal::read_ghostty_keybinds());

    KeymapSnapshot {
        version: SNAPSHOT_VERSION,
        captured_at: now_timestamp(),
        os: std::env::consts::OS.to_string(),
        terminal: detect_terminal_label(),
        terminal_version: std::env::var("TERM_PROGRAM_VERSION").unwrap_or_default(),
        bindings,
    }
}

/// Path of the on-disk keymap snapshot.
pub fn snapshot_path() -> anyhow::Result<std::path::PathBuf> {
    Ok(jcode_storage::jcode_dir()?.join("keymap-snapshot.json"))
}

/// Collect a fresh snapshot and persist it to `~/.jcode/keymap-snapshot.json`.
/// Returns the snapshot regardless of whether the write succeeded.
pub fn refresh_and_save() -> KeymapSnapshot {
    let snapshot = collect_snapshot();
    if let Ok(path) = snapshot_path() {
        if let Err(err) = jcode_storage::write_json(&path, &snapshot) {
            jcode_logging::warn(&format!("keymap snapshot write failed: {err}"));
        }
    }
    snapshot
}

/// Load the last persisted snapshot, if any.
pub fn load_snapshot() -> Option<KeymapSnapshot> {
    let path = snapshot_path().ok()?;
    jcode_storage::read_json(&path).ok()
}

/// Maximum age (seconds) before a cached snapshot is considered stale and
/// refreshed. One day balances freshness against the cost of shelling out.
const SNAPSHOT_MAX_AGE_SECS: u64 = 24 * 60 * 60;

/// Return a usable snapshot, refreshing from the machine only when there is no
/// cached snapshot or the cached one is older than [`SNAPSHOT_MAX_AGE_SECS`].
/// This is the entry point intended for startup: cheap on the common path,
/// self-healing when stale.
pub fn snapshot_cached_or_refresh() -> KeymapSnapshot {
    if let Some(existing) = load_snapshot() {
        if existing.version == SNAPSHOT_VERSION && !snapshot_is_stale(&existing) {
            return existing;
        }
    }
    refresh_and_save()
}

fn snapshot_is_stale(snapshot: &KeymapSnapshot) -> bool {
    use std::time::{SystemTime, UNIX_EPOCH};
    let Ok(captured) = snapshot.captured_at.parse::<u64>() else {
        return true;
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    now.saturating_sub(captured) > SNAPSHOT_MAX_AGE_SECS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_roundtrips_through_json() {
        let snap = KeymapSnapshot {
            version: SNAPSHOT_VERSION,
            captured_at: "123".to_string(),
            os: "macos".to_string(),
            terminal: "Ghostty".to_string(),
            terminal_version: "1.3.1".to_string(),
            bindings: vec![DiscoveredBinding {
                chord: KeyChord::new(true, false, false, false, "k"),
                source: KeySource::Terminal,
                action: "clear_screen".to_string(),
                raw: "super+k=clear_screen".to_string(),
            }],
        };
        let json = serde_json::to_string(&snap).unwrap();
        let back: KeymapSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(back.bindings.len(), 1);
        assert_eq!(back.bindings[0].chord.canonical(), "cmd+k");
        assert_eq!(
            back.from_source(KeySource::Terminal).count(),
            1,
            "should find the terminal binding"
        );
    }
}
