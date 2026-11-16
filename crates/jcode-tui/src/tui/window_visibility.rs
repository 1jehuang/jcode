//! Whether an unfocused client's window can be seen at all.
//!
//! Terminal focus reports give two states, focused and not. A tiling desktop
//! needs a third: an unfocused window may sit next to the focused one in plain
//! view, or on another workspace, in a detached multiplexer session, or behind
//! a display that is switched off. The first should look the same as a focused
//! window. The others can stop animating: nobody can see the spinner, yet it
//! was the largest cost of a background working window (12.5 one-cell repaints
//! a second, each waking the spinner thread and the main thread).
//!
//! Visibility is asked of whatever environment the client runs in, through
//! probes that each answer [`Visibility::Hidden`], [`Visibility::Visible`] or
//! [`Visibility::Unknown`]. Unknown is the default everywhere and is treated as
//! visible, so an environment with no probe behaves exactly as before. Nested
//! environments combine: a pane is hidden if any layer hides it.
//!
//! Probes today: niri (workspace not shown on any output, which also covers a
//! display that is off) and tmux (window not current, or session detached).
//! Sway, Hyprland, X11 and macOS have no probe yet and report Unknown.
//!
//! Probing is done only for an unfocused client with a turn in flight, off the
//! event loop, at most once per [`PROBE_EVERY`]. A focused client never probes.
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Visibility {
    /// No probe could tell. Treated as visible.
    Unknown,
    Visible,
    Hidden,
}

impl Visibility {
    fn code(self) -> u8 {
        match self {
            Visibility::Unknown => 0,
            Visibility::Visible => 1,
            Visibility::Hidden => 2,
        }
    }

    fn from_code(code: u8) -> Self {
        match code {
            1 => Visibility::Visible,
            2 => Visibility::Hidden,
            _ => Visibility::Unknown,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Visibility::Unknown => "unknown",
            Visibility::Visible => "visible",
            Visibility::Hidden => "hidden",
        }
    }

    /// Layers nest (a tmux pane inside a compositor window), so one layer
    /// hiding the pane is enough, and one layer vouching for it is not.
    /// A hidden outer layer already decides the result. Avoid compositor IPC
    /// and process-ancestry reads for a detached/non-current tmux pane.
    fn combine_with(self, other: impl FnOnce() -> Self) -> Self {
        if self == Visibility::Hidden {
            self
        } else {
            self.combine(other())
        }
    }

    fn combine(self, other: Self) -> Self {
        match (self, other) {
            (Visibility::Hidden, _) | (_, Visibility::Hidden) => Visibility::Hidden,
            (Visibility::Visible, _) | (_, Visibility::Visible) => Visibility::Visible,
            _ => Visibility::Unknown,
        }
    }
}

/// How often a hidden working client re-asks. This bounds how long a window
/// that came into view without being focused keeps its slow cadence.
const PROBE_EVERY_HIDDEN: Duration = Duration::from_secs(2);
/// A visible window that becomes hidden only saves power later, so noticing
/// can wait, and a probe costs the compositor a wakeup.
const PROBE_EVERY_VISIBLE: Duration = Duration::from_secs(6);
/// No probe applies in this environment. Re-ask rarely in case it changes
/// (a tmux session is attached from elsewhere).
const PROBE_EVERY_UNKNOWN: Duration = Duration::from_secs(30);
/// A hidden result older than this is ignored, so a probe that stops being refreshed
/// (or hangs) can never leave a window stuck in the hidden cadence.
const FRESH_FOR: Duration = Duration::from_secs(8);
const PROBE_IO_TIMEOUT: Duration = Duration::from_millis(400);

static STATE: AtomicU8 = AtomicU8::new(0);
/// Milliseconds since [`epoch`] of the last finished probe, 0 for none.
static PROBED_AT_MS: AtomicU64 = AtomicU64::new(0);
static IN_FLIGHT: AtomicBool = AtomicBool::new(false);

fn epoch() -> Instant {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    *EPOCH.get_or_init(Instant::now)
}

fn now_ms() -> u64 {
    // Never 0, which means "no probe yet".
    epoch().elapsed().as_millis() as u64 + 1
}

fn age_of_last_probe() -> Option<Duration> {
    match PROBED_AT_MS.load(Ordering::Acquire) {
        0 => None,
        at => Some(Duration::from_millis(now_ms().saturating_sub(at))),
    }
}

/// Keep the visibility of this client's window current. Cheap enough to call
/// on every pass of the event loop: it only starts a probe when one is due.
pub(crate) fn poll(focused: bool, working: bool) {
    if focused {
        // Focus is proof of visibility, and the next time focus is lost the
        // old answer must not be reused.
        if PROBED_AT_MS.load(Ordering::Acquire) != 0 {
            PROBED_AT_MS.store(0, Ordering::Release);
            STATE.store(Visibility::Unknown.code(), Ordering::Release);
        }
        return;
    }
    // Tests share one process and must not depend on the developer's desktop.
    if !working || cfg!(test) {
        return;
    }
    let every = match Visibility::from_code(STATE.load(Ordering::Acquire)) {
        Visibility::Hidden => PROBE_EVERY_HIDDEN,
        Visibility::Visible => PROBE_EVERY_VISIBLE,
        Visibility::Unknown => PROBE_EVERY_UNKNOWN,
    };
    if age_of_last_probe().is_some_and(|age| age < every) {
        return;
    }
    if IN_FLIGHT.swap(true, Ordering::AcqRel) {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("jcode-visibility".to_string())
        .stack_size(256 * 1024)
        .spawn(|| {
            let visibility = probe();
            STATE.store(visibility.code(), Ordering::Release);
            PROBED_AT_MS.store(now_ms(), Ordering::Release);
            IN_FLIGHT.store(false, Ordering::Release);
        });
    if spawned.is_err() {
        IN_FLIGHT.store(false, Ordering::Release);
    }
}

/// Last known visibility. Only meaningful for an unfocused client.
pub(crate) fn current() -> Visibility {
    let state = Visibility::from_code(STATE.load(Ordering::Acquire));
    match age_of_last_probe() {
        None => Visibility::Unknown,
        // Only a hidden answer changes behaviour, so only it can go stale.
        Some(age) if state == Visibility::Hidden && age > FRESH_FOR => Visibility::Unknown,
        Some(_) => state,
    }
}

/// Whether this client is unfocused and known to be out of sight.
pub(crate) fn hidden(focused: bool) -> bool {
    !focused && current() == Visibility::Hidden
}

fn probe() -> Visibility {
    // For tests and for measuring one tier from inside another (a benchmark
    // in a detached tmux session is hidden, yet stands in for a visible pane).
    if let Ok(forced) = std::env::var("JCODE_WINDOW_VISIBILITY") {
        return match forced.trim() {
            "hidden" => Visibility::Hidden,
            "visible" => Visibility::Visible,
            _ => Visibility::Unknown,
        };
    }
    probe_tmux().combine_with(probe_niri)
}

// --- tmux -----------------------------------------------------------------

fn probe_tmux() -> Visibility {
    if std::env::var_os("TMUX").is_none() {
        return Visibility::Unknown;
    }
    let Some(pane) = std::env::var_os("TMUX_PANE") else {
        return Visibility::Unknown;
    };
    let output = std::process::Command::new("tmux")
        .arg("display-message")
        .arg("-p")
        .arg("-t")
        .arg(pane)
        .arg("#{window_active} #{session_attached}")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output();
    match output {
        Ok(output) if output.status.success() => {
            parse_tmux(&String::from_utf8_lossy(&output.stdout))
        }
        _ => Visibility::Unknown,
    }
}

/// `"<window_active> <session_attached>"`: the pane is out of sight when its
/// window is not the session's current one, or no client is attached.
fn parse_tmux(output: &str) -> Visibility {
    let mut fields = output.split_whitespace();
    let (Some(window_active), Some(attached)) = (fields.next(), fields.next()) else {
        return Visibility::Unknown;
    };
    let (Ok(window_active), Ok(attached)) = (window_active.parse::<u32>(), attached.parse::<u32>())
    else {
        return Visibility::Unknown;
    };
    if window_active == 0 || attached == 0 {
        Visibility::Hidden
    } else {
        Visibility::Visible
    }
}

// --- niri -----------------------------------------------------------------

#[cfg(unix)]
fn probe_niri() -> Visibility {
    let Some(socket) = std::env::var_os("NIRI_SOCKET") else {
        return Visibility::Unknown;
    };
    let ancestors = ancestor_pids();
    if ancestors.is_empty() {
        return Visibility::Unknown;
    }
    let socket = std::path::Path::new(&socket);
    let (Some(windows), Some(workspaces)) = (
        niri_request(socket, "\"Windows\"\n"),
        niri_request(socket, "\"Workspaces\"\n"),
    ) else {
        return Visibility::Unknown;
    };
    parse_niri(&windows, &workspaces, &ancestors)
}

#[cfg(not(unix))]
fn probe_niri() -> Visibility {
    Visibility::Unknown
}

#[cfg(unix)]
fn niri_request(socket: &std::path::Path, request: &str) -> Option<serde_json::Value> {
    use std::io::{BufRead, BufReader, Write};
    let mut stream = std::os::unix::net::UnixStream::connect(socket).ok()?;
    stream.set_read_timeout(Some(PROBE_IO_TIMEOUT)).ok()?;
    stream.set_write_timeout(Some(PROBE_IO_TIMEOUT)).ok()?;
    stream.write_all(request.as_bytes()).ok()?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).ok()?;
    serde_json::from_str(&line).ok()
}

/// This process and its parents, nearest first. The terminal emulator that
/// owns the window is one of them when the client runs locally.
#[cfg(target_os = "linux")]
fn ancestor_pids() -> Vec<u32> {
    let mut pids = Vec::new();
    let mut pid = std::process::id();
    while pid > 1 && pids.len() < 32 {
        pids.push(pid);
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
            break;
        };
        // "pid (comm) state ppid ...": comm may contain spaces and parentheses.
        let Some(parent) = stat
            .rsplit_once(')')
            .and_then(|(_, rest)| rest.split_whitespace().nth(1))
            .and_then(|ppid| ppid.parse::<u32>().ok())
        else {
            break;
        };
        pid = parent;
    }
    pids
}

#[cfg(all(unix, not(target_os = "linux")))]
fn ancestor_pids() -> Vec<u32> {
    Vec::new()
}

/// Niri reports each window's workspace and whether a workspace is the one
/// shown on its output. A window is hidden when its workspace is not shown.
/// With the display off no workspace is shown, so everything is hidden.
///
/// One terminal process can own several windows (a single-instance terminal).
/// Which of them holds this client is not known, so the answer is Hidden only
/// if all of them are.
///
/// Not detected: a column scrolled out of view on the shown workspace. Niri
/// 26.04 leaves `tile_pos_in_workspace_view` null, so that case reads Visible.
#[cfg(unix)]
fn parse_niri(
    windows: &serde_json::Value,
    workspaces: &serde_json::Value,
    ancestors: &[u32],
) -> Visibility {
    let (Some(windows), Some(workspaces)) = (
        windows["Ok"]["Windows"].as_array(),
        workspaces["Ok"]["Workspaces"].as_array(),
    ) else {
        return Visibility::Unknown;
    };
    // Nearest ancestor that owns a window: a nested compositor or a terminal
    // started from another terminal must not be mistaken for the owner.
    let Some(owner) = ancestors.iter().copied().find(|pid| {
        windows
            .iter()
            .any(|window| window["pid"].as_u64() == Some(u64::from(*pid)))
    }) else {
        return Visibility::Unknown;
    };
    let mut result = Visibility::Unknown;
    for window in windows {
        if window["pid"].as_u64() != Some(u64::from(owner)) {
            continue;
        }
        if window["is_focused"].as_bool() == Some(true) {
            return Visibility::Visible;
        }
        let Some(workspace_id) = window["workspace_id"].as_u64() else {
            return Visibility::Unknown;
        };
        let Some(shown) = workspaces
            .iter()
            .find(|workspace| workspace["id"].as_u64() == Some(workspace_id))
            .and_then(|workspace| workspace["is_active"].as_bool())
        else {
            return Visibility::Unknown;
        };
        if shown {
            return Visibility::Visible;
        }
        result = Visibility::Hidden;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn lazy_visibility_combination_matches_all_layers_without_redundant_probes() {
        use std::cell::Cell;
        for first in [Visibility::Unknown, Visibility::Visible, Visibility::Hidden] {
            for second in [Visibility::Unknown, Visibility::Visible, Visibility::Hidden] {
                let calls = Cell::new(0);
                let actual = first.combine_with(|| {
                    calls.set(calls.get() + 1);
                    second
                });
                assert_eq!(actual, first.combine(second));
                assert_eq!(calls.get(), usize::from(first != Visibility::Hidden));
            }
        }
    }

    #[test]
    fn layers_combine_towards_hidden_and_unknown_stays_unknown() {
        use Visibility::*;
        assert_eq!(Unknown.combine(Unknown), Unknown);
        assert_eq!(Unknown.combine(Visible), Visible);
        assert_eq!(Visible.combine(Hidden), Hidden);
        assert_eq!(Hidden.combine(Unknown), Hidden);
    }

    #[test]
    fn tmux_pane_is_hidden_when_its_window_is_not_current_or_nothing_is_attached() {
        assert_eq!(parse_tmux("1 1\n"), Visibility::Visible);
        assert_eq!(parse_tmux("1 2\n"), Visibility::Visible);
        assert_eq!(parse_tmux("0 1\n"), Visibility::Hidden);
        assert_eq!(parse_tmux("1 0\n"), Visibility::Hidden);
        // Anything unexpected must not hide a window.
        assert_eq!(parse_tmux(""), Visibility::Unknown);
        assert_eq!(parse_tmux("1\n"), Visibility::Unknown);
        assert_eq!(parse_tmux("yes no\n"), Visibility::Unknown);
    }

    fn workspaces(shown: &[u64]) -> serde_json::Value {
        json!({"Ok": {"Workspaces": [
            {"id": 1, "is_active": shown.contains(&1)},
            {"id": 2, "is_active": shown.contains(&2)},
        ]}})
    }

    fn windows(entries: &[(u64, u64, bool)]) -> serde_json::Value {
        let windows: Vec<_> = entries
            .iter()
            .map(|(pid, workspace, focused)| {
                json!({"pid": pid, "workspace_id": workspace, "is_focused": focused})
            })
            .collect();
        json!({"Ok": {"Windows": windows}})
    }

    #[test]
    fn niri_window_on_a_workspace_that_is_not_shown_is_hidden() {
        let ancestors = [900, 500, 100];
        let all = windows(&[(500, 2, false), (777, 1, true)]);
        assert_eq!(
            parse_niri(&all, &workspaces(&[1]), &ancestors),
            Visibility::Hidden
        );
        assert_eq!(
            parse_niri(&all, &workspaces(&[1, 2]), &ancestors),
            Visibility::Visible
        );
        // Display off: niri shows no workspace anywhere.
        assert_eq!(
            parse_niri(&windows(&[(500, 1, false)]), &workspaces(&[]), &ancestors),
            Visibility::Hidden
        );
    }

    #[test]
    fn niri_single_instance_terminal_is_hidden_only_if_all_its_windows_are() {
        let ancestors = [900, 500];
        assert_eq!(
            parse_niri(
                &windows(&[(500, 2, false), (500, 1, false)]),
                &workspaces(&[1]),
                &ancestors
            ),
            Visibility::Visible
        );
        assert_eq!(
            parse_niri(
                &windows(&[(500, 2, false), (500, 2, false)]),
                &workspaces(&[1]),
                &ancestors
            ),
            Visibility::Hidden
        );
    }

    #[test]
    fn niri_answers_unknown_rather_than_guess() {
        let shown = workspaces(&[1]);
        // No ancestor owns a window: the client runs over ssh or in a nested
        // session whose terminal lives elsewhere.
        assert_eq!(
            parse_niri(&windows(&[(777, 2, false)]), &shown, &[900, 500]),
            Visibility::Unknown
        );
        // Unknown workspace, error reply, or a changed schema.
        assert_eq!(
            parse_niri(&windows(&[(500, 9, false)]), &shown, &[500]),
            Visibility::Unknown
        );
        assert_eq!(
            parse_niri(&json!({"Err": "nope"}), &shown, &[500]),
            Visibility::Unknown
        );
        assert_eq!(
            parse_niri(&json!({"Ok": {"Windows": [{"pid": 500}]}}), &shown, &[500]),
            Visibility::Unknown
        );
        // The nearest window-owning ancestor decides, not an outer one.
        assert_eq!(
            parse_niri(
                &windows(&[(500, 2, false), (100, 1, false)]),
                &shown,
                &[900, 500, 100]
            ),
            Visibility::Hidden
        );
    }

    #[test]
    fn nothing_is_hidden_without_a_fresh_probe() {
        // Tests never probe, so the process-wide answer stays Unknown.
        poll(false, true);
        assert_eq!(current(), Visibility::Unknown);
        assert!(!hidden(false));
        assert!(!hidden(true));
    }
}
