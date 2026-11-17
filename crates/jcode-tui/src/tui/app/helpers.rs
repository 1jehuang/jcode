#![cfg_attr(test, allow(clippy::items_after_test_module))]

mod clipboard_helper;
mod git_line_cache;
#[cfg_attr(test, allow(dead_code))]
mod git_watch;
pub(crate) mod model_names;

use crate::todo::TodoItem;
#[path = "helpers_environment.rs"]
mod environment;
pub(super) use environment::clipboard_image;
use environment::gather_ambient_info_inner;
pub(crate) use environment::gather_git_info_in;

use crate::tui::info_widget::{AmbientWidgetData, GitInfo};
use crate::tui::session_picker::ResumeTarget;
use crossterm::event::{KeyCode, KeyModifiers};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

type AmbientInfoCacheEntry = (std::time::Instant, bool, Option<AmbientWidgetData>, bool);

static AMBIENT_INFO_CACHE: Mutex<Option<AmbientInfoCacheEntry>> = Mutex::new(None);

/// Stale-while-revalidate cache for the git status widget. Module-level so the
/// app can force a refresh the moment it mutates the repo (commit, shell, file
/// edits) instead of waiting out the TTL with a stale branch/dirty count.
type GitInfoCacheEntry = (std::time::Instant, Option<GitInfo>, bool);
static GIT_INFO_CACHE: Mutex<Option<GitInfoCacheEntry>> = Mutex::new(None);
static GIT_LINE_CACHE: std::sync::LazyLock<Mutex<git_line_cache::GitLineCache>> =
    std::sync::LazyLock::new(|| Mutex::new(git_line_cache::GitLineCache::default()));
/// Cheap `.git` metadata stamp from the last full probe, plus when that probe
/// ran. While the stamp is unchanged, TTL expiry only re-stats a few files
/// instead of spawning six git processes every 5s per client.
static GIT_PROBE_STAMP: Mutex<Option<(std::time::Instant, GitStamp)>> = Mutex::new(None);

/// Full probes still run this often with an unchanged stamp, because editor
/// writes to tracked files do not touch `.git` until something stages them.
#[cfg(not(test))]
const GIT_FULL_PROBE_INTERVAL: Duration = Duration::from_secs(60);

type GitStamp = Vec<(std::path::PathBuf, Option<(std::time::SystemTime, u64)>)>;

/// mtimes of the files that change on commit, checkout, stage, fetch, and
/// branch updates. `None` when the git dir cannot be resolved cheaply.
fn git_stamp(repo_root: &std::path::Path) -> Option<GitStamp> {
    let git_dir = repo_root.join(".git");
    if !git_dir.is_dir() {
        return None;
    }
    let mut paths = vec![
        git_dir.join("HEAD"),
        git_dir.join("index"),
        git_dir.join("FETCH_HEAD"),
        git_dir.join("packed-refs"),
    ];
    if let Ok(head) = std::fs::read_to_string(git_dir.join("HEAD"))
        && let Some(reference) = head.trim().strip_prefix("ref: ")
    {
        paths.push(git_dir.join(reference));
        if let Some(branch) = reference.strip_prefix("refs/heads/") {
            paths.push(git_dir.join("refs/remotes/origin").join(branch));
        }
    }
    Some(
        paths
            .into_iter()
            .map(|path| {
                let meta = match std::fs::metadata(&path).and_then(|m| Ok((m.modified()?, m.len())))
                {
                    Ok(meta) => Some(meta),
                    Err(error) => {
                        if error.kind() != std::io::ErrorKind::NotFound {
                            crate::logging::debug(&format!(
                                "Git metadata probe failed for {}: {error}",
                                path.display()
                            ));
                        }
                        None
                    }
                };
                (path, meta)
            })
            .collect(),
    )
}

/// Stale-while-revalidate cache for per-session todos plus their goal-level
/// assessments (closed feedback loop etc.). Module-level so the app can force a
/// refresh the moment it persists a todo write locally, instead of showing
/// the previous list until the TTL lapses.
type TodosCacheEntry = (
    std::time::Instant,
    Vec<TodoItem>,
    Vec<crate::todo::TodoGoal>,
    bool,
);
type TodosCache = std::collections::HashMap<String, TodosCacheEntry>;
static TODOS_CACHE: std::sync::LazyLock<Mutex<TodosCache>> =
    std::sync::LazyLock::new(|| Mutex::new(std::collections::HashMap::new()));

/// Backdate `Instant::now()` by up to `amount`, saturating instead of
/// panicking when the clock's epoch is too recent.
///
/// `Instant` counts from boot on Windows (QPC) and Linux (CLOCK_MONOTONIC), so
/// `Instant::now() - one_hour` panics with "overflow when subtracting duration
/// from instant" when the machine booted more recently than that. This hit
/// real users right after a reboot: the git cache invalidation below runs
/// after every bash/edit tool, crashing the whole TUI (issue #424).
pub(crate) fn backdated_now(amount: Duration) -> std::time::Instant {
    let now = std::time::Instant::now();
    let mut backdate = amount;
    loop {
        if let Some(instant) = now.checked_sub(backdate) {
            return instant;
        }
        if backdate < Duration::from_millis(1) {
            return now;
        }
        backdate /= 2;
    }
}

/// Force the git-status widget cache to refetch on its next read.
///
/// Call this right after the app changes the working tree or HEAD (commits,
/// shell commands, file edits) so the info widget reflects the new repo state
/// immediately rather than after the 5s TTL. Stale-while-revalidate still
/// applies: the next read returns the last value and kicks a background refresh.
pub(crate) fn invalidate_git_info_cache() {
    crate::tui::cloud_git::refresh_soon();
    if let Ok(mut guard) = GIT_INFO_CACHE.lock()
        && let Some((ts, _cached, refreshing)) = guard.as_mut()
    {
        // Backdate the timestamp past the TTL so the next `gather_git_info`
        // treats the entry as expired and spawns a refresh, while still
        // returning the last-known value (no flicker to empty).
        *ts = backdated_now(Duration::from_secs(3600));
        *refreshing = false;
    }
    if let Ok(mut stamp) = GIT_PROBE_STAMP.lock() {
        *stamp = None;
    }
}

/// Pin the git-status widget to a fixed value for deterministic renders.
///
/// Full-frame artifact generators (onboarding screenshots) would otherwise
/// capture the live ahead/behind/dirty counts of whatever repo the generator
/// happens to run in. Marking the entry as `refreshing` keeps the TTL path
/// from spawning a background probe that overwrites the seed mid-render.
#[cfg(test)]
pub(crate) fn seed_git_info_cache_for_tests(info: Option<GitInfo>) {
    if let Ok(mut guard) = GIT_INFO_CACHE.lock() {
        *guard = Some((std::time::Instant::now(), info, true));
    }
}

/// Force the todos widget cache to refetch the given session on its next read.
///
/// Call this right after the app persists a local todo write so the info widget
/// reflects the new list immediately rather than after the 1s TTL.
pub(crate) fn invalidate_todos_cache(session_id: &str) {
    if let Ok(mut cache) = TODOS_CACHE.lock()
        && let Some((ts, _todos, _goals, refreshing)) = cache.get_mut(session_id)
    {
        *ts = backdated_now(Duration::from_secs(3600));
        *refreshing = false;
    }
}

/// Force the ambient widget cache to refetch on its next read.
///
/// Call this after the app changes ambient state (e.g. the `schedule` tool
/// queues or cancels a task) so the ambient panel reflects the new queue/next
/// wake immediately rather than after the 2s TTL.
pub(crate) fn invalidate_ambient_info_cache() {
    if let Ok(mut guard) = AMBIENT_INFO_CACHE.lock()
        && let Some((ts, _enabled, _cached, refreshing)) = guard.as_mut()
    {
        *ts = backdated_now(Duration::from_secs(3600));
        *refreshing = false;
    }
}

/// Open a file/URL with the system opener, unless suppressed.
///
/// Every TUI-initiated `open::that_detached` must go through here: it honors
/// NO_BROWSER/JCODE_NO_BROWSER and refuses to open anything from test binaries
/// (`browser_suppressed` detects the test harness), so `cargo test` runs never
/// pop browser windows, image viewers, or OAuth pages on the developer's
/// desktop.
pub(crate) fn open_path_or_url_detached(
    target: impl AsRef<std::ffi::OsStr>,
) -> std::io::Result<()> {
    if crate::tui::is_ssh_remote() {
        let target = target.as_ref().to_string_lossy();
        if !target.starts_with("https://") && !target.starts_with("http://") {
            return Err(std::io::Error::other(
                "remote file opening is unavailable over SSH; open it on the remote host or ask the agent to read it",
            ));
        }
    }
    if crate::auth::browser_suppressed(false) {
        return Err(std::io::Error::other(
            "opening files/URLs is suppressed (NO_BROWSER/JCODE_NO_BROWSER or test harness)",
        ));
    }
    open::that_detached(target)
}

/// Test-only: snapshot `(elapsed_secs, refreshing)` for a session's todos cache
/// entry, or `None` when no entry exists yet. Lets tests assert that
/// invalidation backdates the entry so the next gather treats it as expired.
#[cfg(test)]
pub(crate) fn todos_cache_entry_age_for_tests(session_id: &str) -> Option<(u64, bool)> {
    let cache = TODOS_CACHE.lock().ok()?;
    cache
        .get(session_id)
        .map(|(ts, _todos, _goals, refreshing)| (ts.elapsed().as_secs(), *refreshing))
}

/// Test-only: clear the entire todos cache so tests start from a known state.
#[cfg(test)]
pub(crate) fn clear_todos_cache_for_tests() {
    if let Ok(mut cache) = TODOS_CACHE.lock() {
        cache.clear();
    }
}

#[derive(Clone)]
pub(super) struct CachedContextSnapshot {
    pub session_key: String,
    pub is_remote: bool,
    pub display_messages_version: u64,
    pub context_revision: u64,
    pub message_count: usize,
    pub compaction_count: usize,
    pub compaction_summary_chars: usize,
    pub is_compacting: bool,
    pub snapshot: crate::tui::ContextSnapshot,
}

pub(super) fn extract_bracketed_system_message(message: &str) -> Option<String> {
    let trimmed = message.trim();
    let body = trimmed.strip_prefix("[SYSTEM:")?.trim_start();
    let body = body.strip_suffix(']').unwrap_or(body).trim();
    if body.is_empty() {
        None
    } else {
        Some(body.to_string())
    }
}

pub(super) fn launch_client_executable() -> PathBuf {
    crate::build::client_update_candidate(jcode_selfdev_types::client_selfdev_requested())
        .map(|(path, _label)| path)
        .or_else(|| std::env::current_exe().ok())
        .unwrap_or_else(|| PathBuf::from("jcode"))
}

pub(super) fn partition_queued_messages(
    messages: Vec<String>,
    reminders: Vec<String>,
) -> (Vec<String>, Option<String>, Vec<String>) {
    let mut user_messages = Vec::new();
    let mut display_system_messages = Vec::new();
    let mut reminder_parts = reminders;

    for message in messages {
        if let Some(system_message) = extract_bracketed_system_message(&message) {
            reminder_parts.push(system_message.clone());
            display_system_messages.push(system_message);
        } else {
            user_messages.push(message);
        }
    }

    let reminder = if reminder_parts.is_empty() {
        None
    } else {
        Some(reminder_parts.join("\n\n"))
    };

    (user_messages, reminder, display_system_messages)
}

#[cfg(target_os = "macos")]
pub(super) fn ctrl_bracket_fallback_to_esc(code: &mut KeyCode, modifiers: &mut KeyModifiers) {
    if !modifiers.contains(KeyModifiers::CONTROL) {
        return;
    }
    if *code == KeyCode::Esc {
        *code = KeyCode::Char('[');
    }
}

#[cfg(not(target_os = "macos"))]
pub(super) fn ctrl_bracket_fallback_to_esc(_code: &mut KeyCode, _modifiers: &mut KeyModifiers) {}

/// Debug command file path
pub(super) fn debug_cmd_path() -> PathBuf {
    if let Ok(path) = std::env::var("JCODE_DEBUG_CMD_PATH") {
        return PathBuf::from(path);
    }
    std::env::temp_dir().join("jcode_debug_cmd")
}

/// Debug response file path
pub(super) fn debug_response_path() -> PathBuf {
    if let Ok(path) = std::env::var("JCODE_DEBUG_RESPONSE_PATH") {
        return PathBuf::from(path);
    }
    std::env::temp_dir().join("jcode_debug_response")
}

#[path = "helpers_rate_limit_parse.rs"]
mod rate_limit_parse;
pub(super) use rate_limit_parse::{MAX_TURN_HOLD_SECS, parse_rate_limit_error};

pub(super) fn is_context_limit_error(error: &str) -> bool {
    if crate::provider::openai_request::is_openai_encrypted_content_too_large_error(error) {
        return true;
    }
    let lower = error.to_lowercase();
    lower.contains("context length")
        || lower.contains("context window")
        || lower.contains("maximum context")
        || lower.contains("max context")
        || lower.contains("token limit")
        || lower.contains("too many tokens")
        || lower.contains("prompt is too long")
        || lower.contains("input is too long")
        || lower.contains("request too large")
        || lower.contains("length limit")
        || lower.contains("maximum tokens")
        || (lower.contains("exceeded") && lower.contains("tokens"))
}

/// Whether `error` is a provider HTTP 413 "request too large" / payload-size
/// rejection. This is distinct from a token-context overflow: it is driven by
/// the serialized request body size (dominated by inline base64 images), so it
/// is recovered by stripping oversized images rather than by token compaction.
pub(super) fn is_request_payload_too_large_error(error: &str) -> bool {
    crate::compaction::is_request_payload_too_large_error(error)
}

/// Parse a clock time like "5am" or "12:30pm" and return duration until that time
pub(super) fn parse_clock_time_to_duration(time_str: &str) -> Option<Duration> {
    let time_lower = time_str.to_lowercase();
    let is_pm = time_lower.ends_with("pm");
    let time_part = time_lower.trim_end_matches("am").trim_end_matches("pm");

    let (hour, minute) = if time_part.contains(':') {
        let parts: Vec<&str> = time_part.split(':').collect();
        if parts.len() != 2 {
            return None;
        }
        let h: u32 = parts[0].parse().ok()?;
        let m: u32 = parts[1].parse().ok()?;
        (h, m)
    } else {
        let h: u32 = time_part.parse().ok()?;
        (h, 0)
    };

    let hour_24 = if is_pm && hour != 12 {
        hour + 12
    } else if !is_pm && hour == 12 {
        0
    } else {
        hour
    };

    if hour_24 >= 24 || minute >= 60 {
        return None;
    }

    let now = chrono::Local::now();
    let today = now.date_naive();
    let target_time = chrono::NaiveTime::from_hms_opt(hour_24, minute, 0)?;
    let mut target_datetime = today.and_time(target_time);

    if target_datetime <= now.naive_local() {
        target_datetime = (today + chrono::Duration::days(1)).and_time(target_time);
    }

    let duration_secs = (target_datetime - now.naive_local()).num_seconds();
    if duration_secs > 0 {
        Some(Duration::from_secs(duration_secs as u64))
    } else {
        None
    }
}

pub(super) fn format_cache_footer(
    read_tokens: Option<u64>,
    write_tokens: Option<u64>,
) -> Option<String> {
    let _ = (read_tokens, write_tokens);
    None
}

/// Format token count for display (e.g., 63000 -> "63K")
pub(super) fn format_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.0}k", tokens as f64 / 1_000.0)
    } else {
        format!("{}", tokens)
    }
}

/// Test-only clipboard sink.
///
/// A headless CI runner has no Wayland socket, no X11 display, and a
/// non-terminal stdout, so every real clipboard path correctly fails and
/// `copy_to_clipboard` returns false. Tests that only care about shortcut
/// wiring (does Alt+S reach the copy handler with the right text?) then fail
/// for an environment reason rather than a code reason. Capturing into this
/// sink lets those tests assert the wiring *and* the copied text without
/// depending on a desktop session (refs #596).
#[cfg(test)]
static TEST_CLIPBOARD: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// Test-only: route clipboard writes into an in-process sink instead of the OS.
#[cfg(test)]
pub(crate) fn capture_clipboard_for_tests() {
    if let Ok(mut sink) = TEST_CLIPBOARD.lock() {
        *sink = Some(String::new());
    }
}

/// Test-only: the last text written while capture was enabled.
#[cfg(test)]
pub(crate) fn captured_clipboard_for_tests() -> Option<String> {
    TEST_CLIPBOARD.lock().ok().and_then(|sink| sink.clone())
}

/// Test-only: stop capturing and drop any captured text.
#[cfg(test)]
pub(crate) fn stop_capturing_clipboard_for_tests() {
    if let Ok(mut sink) = TEST_CLIPBOARD.lock() {
        *sink = None;
    }
}

/// True when this process is running inside an SSH session, i.e. the user's
/// machine (and clipboard) is elsewhere. In that case the *local* clipboard
/// belongs to the server, and the only path to the user's clipboard is an
/// OSC 52 sequence emitted through the terminal (and forwarded by the SSH
/// client to the client-side terminal emulator).
#[cfg_attr(test, allow(dead_code))]
fn is_remote_ssh_session() -> bool {
    std::env::var("SSH_CONNECTION").is_ok() || std::env::var("SSH_TTY").is_ok()
}

/// Runs the session-appropriate copy chain and reports its outcome.
///
/// The *order* is the contract (it is what the original bug violated):
///
/// - **Remote (SSH) session**: OSC 52 runs first, because it is the only path
///   to the user's (client-side) clipboard, and the native server-side
///   clipboard (NSPasteboard / Win32) would otherwise succeed headless and
///   short-circuit the chain. The local clipboard is then also filled so
///   paste inside jcode keeps working. If OSC 52 is unavailable (e.g. stdout
///   is not a TTY), the local clipboard is still the fallback.
/// - **Local session**: the native clipboard is authoritative and OSC 52 only
///   runs if it fails (a working native clipboard must not emit terminal
///   escape sequences).
///
/// The writers are closures so the order can be unit-tested with stubs,
/// without touching any real clipboard.
fn run_copy_chain(
    remote: bool,
    mut osc52: impl FnMut() -> bool,
    mut local: impl FnMut() -> bool,
) -> bool {
    if remote {
        if osc52() {
            // Keep the local clipboard in sync so paste inside jcode
            // (which reads the server-side clipboard) keeps working.
            let _ = local();
            return true;
        }
        // OSC 52 did not go through (e.g. stdout is not a TTY): still try the
        // local clipboard rather than failing outright.
        return local();
    }
    if local() {
        return true;
    }
    osc52()
}

#[cfg(test)]
mod copy_chain_tests {
    use super::run_copy_chain;
    use std::cell::RefCell;

    /// Records the order in which the clipboard writers are invoked, so the
    /// ordering contract of [`run_copy_chain`] is pinned by tests: a
    /// regression that writes the server-side clipboard before OSC 52 in an
    /// SSH session (the original bug) fails here.
    struct OrderLog(RefCell<Vec<&'static str>>);

    impl OrderLog {
        fn writer(&self, label: &'static str, succeeds: bool) -> impl FnMut() -> bool + '_ {
            let log = &self.0;
            move || {
                log.borrow_mut().push(label);
                succeeds
            }
        }

        fn events(&self) -> Vec<&'static str> {
            self.0.borrow().clone()
        }
    }

    #[test]
    fn ssh_session_writes_osc52_first_then_local() {
        let log = OrderLog(RefCell::new(Vec::new()));
        assert!(run_copy_chain(
            true,
            log.writer("osc52", true),
            log.writer("local", true),
        ));
        assert_eq!(log.events(), ["osc52", "local"]);
    }

    #[test]
    fn ssh_session_still_writes_local_when_osc52_fails() {
        let log = OrderLog(RefCell::new(Vec::new()));
        assert!(run_copy_chain(
            true,
            log.writer("osc52", false),
            log.writer("local", true),
        ));
        assert_eq!(log.events(), ["osc52", "local"]);
    }

    #[test]
    fn ssh_session_reports_failure_when_both_writers_fail() {
        let log = OrderLog(RefCell::new(Vec::new()));
        assert!(!run_copy_chain(
            true,
            log.writer("osc52", false),
            log.writer("local", false),
        ));
        assert_eq!(log.events(), ["osc52", "local"]);
    }

    #[test]
    fn local_session_prefers_native_clipboard_and_skips_osc52() {
        let log = OrderLog(RefCell::new(Vec::new()));
        assert!(run_copy_chain(
            false,
            log.writer("osc52", true),
            log.writer("local", true),
        ));
        // A working native clipboard must not emit terminal escape sequences.
        assert_eq!(log.events(), ["local"]);
    }

    #[test]
    fn local_session_falls_back_to_osc52_when_native_fails() {
        let log = OrderLog(RefCell::new(Vec::new()));
        assert!(run_copy_chain(
            false,
            log.writer("osc52", true),
            log.writer("local", false),
        ));
        assert_eq!(log.events(), ["local", "osc52"]);
    }

    #[test]
    fn local_session_reports_failure_when_both_writers_fail() {
        let log = OrderLog(RefCell::new(Vec::new()));
        assert!(!run_copy_chain(
            false,
            log.writer("osc52", false),
            log.writer("local", false),
        ));
        assert_eq!(log.events(), ["local", "osc52"]);
    }
}

/// Fill the *local* (server-side) clipboard using the platform-native chain.
///
/// - Windows: Win32 via arboard, which is authoritative there (issue #497).
/// - macOS: NSPasteboard via arboard, with pbcopy as a belt-and-braces
///   fallback (Terminal.app ignores OSC 52, so the native path must win
///   locally).
/// - Linux: wl-copy (Wayland), then xclip/xsel (X11, which fork and keep
///   owning the selection unlike arboard, whose X11 backend reverts the
///   selection on drop - issue #684), then arboard.
#[cfg(not(test))]
fn copy_to_local_clipboard(text: &str) -> bool {
    #[cfg(windows)]
    {
        arboard::Clipboard::new()
            .and_then(|mut cb| cb.set_text(text.to_string()))
            .is_ok()
    }

    #[cfg(target_os = "macos")]
    {
        copy_to_local_macos_clipboard(text)
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    {
        if clipboard_helper::copy_via_clipboard_helper("wl-copy", &[], text) {
            return true;
        }
        if clipboard_helper::copy_via_clipboard_helper("xclip", &["-selection", "clipboard"], text)
        {
            return true;
        }
        if clipboard_helper::copy_via_clipboard_helper("xsel", &["--clipboard", "--input"], text) {
            return true;
        }
        arboard::Clipboard::new()
            .and_then(|mut cb| cb.set_text(text.to_string()))
            .is_ok()
    }
}

#[cfg(all(not(test), target_os = "macos"))]
fn copy_to_local_macos_clipboard(text: &str) -> bool {
    if arboard::Clipboard::new()
        .and_then(|mut cb| cb.set_text(text.to_string()))
        .is_ok()
    {
        return true;
    }
    if let Ok(mut child) = std::process::Command::new("pbcopy")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        use std::io::Write;
        if let Some(stdin) = child.stdin.as_mut()
            && stdin.write_all(text.as_bytes()).is_ok()
        {
            drop(child.stdin.take());
            if child.wait().map(|s| s.success()).unwrap_or(false) {
                return true;
            }
        }
    }
    false
}

/// Copy text to clipboard.
///
/// The ordering contract lives in [`run_copy_chain`]: in a remote (SSH)
/// session OSC 52 is emitted first, because the platform-native clipboards
/// succeed even headless and would otherwise short-circuit the only path to
/// the user's own clipboard; the local clipboard is then also filled so
/// paste inside jcode keeps working. In a local session the native clipboard
/// is authoritative and OSC 52 is the final fallback (see
/// [`copy_to_local_clipboard`] for the per-platform native chain).
pub(crate) fn copy_to_clipboard(text: &str) -> bool {
    // Under test, never touch the OS clipboard. Beyond making results identical
    // on a desktop and a headless runner, the Linux path below spawns `wl-copy`,
    // which forks a clipboard server that does not exit; waiting on it hangs the
    // test binary indefinitely. Tests that assert copied text call
    // `capture_clipboard_for_tests` first and then read the sink; tests that
    // only assert "a copy happened" get a truthy result either way.
    //
    // The OS paths are cfg'd out (not merely skipped) so the test build does
    // not carry an unreachable tail after this block's `return`.
    #[cfg(test)]
    {
        if let Ok(mut sink) = TEST_CLIPBOARD.lock() {
            match sink.as_mut() {
                Some(captured) => {
                    captured.clear();
                    captured.push_str(text);
                }
                None => *sink = Some(text.to_string()),
            }
        }
        true
    }

    #[cfg(not(test))]
    {
        run_copy_chain(
            is_remote_ssh_session(),
            || copy_to_clipboard_osc52(text),
            || copy_to_local_clipboard(text),
        )
    }
}

/// Copy text to the clipboard using the OSC 52 terminal escape sequence.
#[cfg_attr(test, allow(dead_code))]
fn copy_to_clipboard_osc52(text: &str) -> bool {
    use base64::Engine as _;
    use std::io::{IsTerminal, Write};

    let mut out = std::io::stdout();
    if !out.is_terminal() {
        return false;
    }
    let encoded = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
    // OSC 52: ESC ] 52 ; c ; <base64> BEL
    let seq = format!("\x1b]52;c;{}\x07", encoded);
    out.write_all(seq.as_bytes()).is_ok() && out.flush().is_ok()
}

pub(crate) fn effort_display_label(effort: &str) -> &str {
    effort_display_label_with_root(effort, crate::prompt::swarm_root_reasoning_effort(effort))
}

// Keep finite, validated effort labels static so autocomplete can share them
// without allocations or leaking dynamically formatted strings.
fn effort_display_label_with_root<'a>(effort: &'a str, root: Option<&str>) -> &'a str {
    macro_rules! swarm_label {
        ($mode:literal, $detail:literal) => {
            match root.unwrap_or("low") {
                "none" => concat!($mode, " (None + ", $detail, ") [Beta]"),
                "minimal" => concat!($mode, " (Minimal + ", $detail, ") [Beta]"),
                "low" => concat!($mode, " (Low + ", $detail, ") [Beta]"),
                "medium" => concat!($mode, " (Medium + ", $detail, ") [Beta]"),
                "high" => concat!($mode, " (High + ", $detail, ") [Beta]"),
                "xhigh" => concat!($mode, " (xHigh + ", $detail, ") [Beta]"),
                _ => concat!($mode, " (Max + ", $detail, ") [Beta]"),
            }
        };
    }
    match effort {
        "swarm" => swarm_label!("Swarm", "parallel agents"),
        "max" => "Max",
        "xhigh" => "xHigh",
        "high" => "High",
        "medium" => "Medium",
        "low" => "Low",
        "none" => "None",
        other => other,
    }
}

pub(super) fn inferred_reasoning_efforts(
    provider_name: Option<&str>,
    model_name: Option<&str>,
) -> Vec<&'static str> {
    jcode_provider_core::inferred_reasoning_efforts(provider_name, model_name)
}

pub(super) fn effort_bar(index: usize, total: usize) -> String {
    let mut bar = String::new();
    for i in 0..total {
        if i == index {
            bar.push('●');
        } else {
            bar.push('○');
        }
    }
    bar
}

/// Status line for the speed-tier hotkey, e.g. "Speed: Fast ○●○".
/// `at_end` carries the attempted direction when the tier could not move.
pub(super) fn speed_tier_notice(
    tier: &str,
    index: usize,
    total: usize,
    at_end: Option<i8>,
) -> String {
    let label = service_tier_display_label(tier);
    let bar = effort_bar(index, total);
    match at_end {
        Some(direction) => format!(
            "Speed: {} {} (already at {})",
            label,
            bar,
            if direction > 0 { "max" } else { "min" }
        ),
        None => format!("Speed: {} {}", label, bar),
    }
}

/// True when the active tier is any accelerated tier (Fast or Ultrafast).
pub(super) fn service_tier_is_fast(service_tier: Option<&str>) -> bool {
    matches!(service_tier, Some("priority" | "fast" | "ultrafast"))
}

pub(super) fn service_tier_display_label(service_tier: &str) -> &str {
    match service_tier {
        "priority" | "fast" => "Fast",
        "ultrafast" => "Ultrafast",
        "flex" => "Flex",
        // Explicit disable values persisted by "/fast default off" (issue
        // #506) and accepted by the OpenAI runtime.
        "off" | "default" | "auto" | "none" => "Standard",
        other => other,
    }
}

pub(super) fn fast_mode_success_message(
    enabled: bool,
    label: &str,
    applies_next_request: bool,
) -> String {
    let status = if enabled { "on" } else { "off" };
    if applies_next_request {
        format!(
            "✓ Fast mode configured {} ({})\nRequested for the next provider request, including later in this turn. The current in-flight request is unchanged. Provider acceptance is not yet confirmed.",
            status, label
        )
    } else {
        format!(
            "✓ Fast mode configured {} ({})\nRequested for the next provider request. Provider acceptance is not yet confirmed.",
            status, label
        )
    }
}

pub(super) fn fast_mode_status_notice(
    service_tier: Option<&str>,
    applies_next_request: bool,
) -> String {
    let status = match service_tier {
        Some("ultrafast") => "ultra",
        tier if service_tier_is_fast(tier) => "on",
        _ => "off",
    };
    if applies_next_request {
        format!("Fast: {} (next request)", status)
    } else {
        format!("Fast: {}", status)
    }
}

pub(super) fn fast_mode_overview_message(
    enabled: bool,
    current_label: &str,
    default_enabled: bool,
    default_label: &str,
) -> String {
    format!(
        "Fast mode is {}.\nCurrent tier: {}\nSaved default: {} ({})\nUse /fast on, /fast off, or /fast default on|off.",
        if enabled { "on" } else { "off" },
        current_label,
        if default_enabled { "on" } else { "off" },
        default_label
    )
}

pub(super) fn fast_mode_default_message(default_enabled: bool, default_label: &str) -> String {
    format!(
        "Saved default fast mode is {}.\nDefault tier: {}\nUse /fast default on or /fast default off.",
        if default_enabled { "on" } else { "off" },
        default_label
    )
}

/// Suffix for "Switched to model" notices: auth method plus the masked
/// account email (e.g. `j***y@gmail.com`) when the route uses OAuth, so the
/// account is recognizable without exposing the full address on screen.
pub(crate) fn switched_model_account_suffix(
    auth_label: Option<&str>,
    oauth_email: Option<&str>,
) -> String {
    let email = oauth_email
        .map(str::trim)
        .filter(|email| !email.is_empty())
        .map(mask_email);
    match (auth_label, email) {
        (Some(label), Some(email)) => format!(" (via {label} · {email})"),
        (Some(label), None) => format!(" (via {label})"),
        (None, Some(email)) => format!(" ({email})"),
        (None, None) => String::new(),
    }
}

/// Notice after switching the active saved account. The switch rebinds this
/// session and becomes the default for new sessions, but other sessions that
/// are already running keep the credentials they loaded.
#[cfg(test)]
pub(super) fn account_switch_notice(provider: &str, label: &str) -> String {
    format!(
        "Switched {provider} account to `{label}` for this session and new sessions. \
         Other running sessions keep their current account until they reload."
    )
}

pub(super) fn mask_email(email: &str) -> String {
    let trimmed = email.trim();
    let Some((local, domain)) = trimmed.split_once('@') else {
        return trimmed.to_string();
    };

    if local.is_empty() {
        return format!("***@{}", domain);
    }

    let mut chars = local.chars();
    let first = chars.next().unwrap_or('*');
    let last = chars.last().unwrap_or(first);

    let masked_local = if local.chars().count() <= 2 {
        format!("{}*", first)
    } else {
        format!("{}***{}", first, last)
    };

    format!("{}@{}", masked_local, domain)
}

/// Spawn a new terminal window that resumes a jcode session.
/// Returns Ok(true) if a terminal was successfully launched, Ok(false) if no terminal found.
fn resume_invocation_args(session_id: &str, socket: Option<&str>) -> Vec<String> {
    if let Some(lease) = crate::storage::read_session_lease(session_id)
        && let Some(host) = lease.away_host
    {
        return remote_resume_invocation_args(session_id, &host, lease.repo_root.as_deref());
    }
    let mut args = vec![
        "--fresh-spawn".to_string(),
        "--resume".to_string(),
        session_id.to_string(),
    ];
    if let Some(socket) = socket.filter(|s| !s.trim().is_empty()) {
        args.push("--socket".to_string());
        args.push(socket.to_string());
    }
    args
}

fn remote_resume_invocation_args(
    session_id: &str,
    host: &str,
    working_dir: Option<&str>,
) -> Vec<String> {
    let mut args = vec!["--fresh-spawn".into(), "--ssh".into(), host.into()];
    if let Ok(binary) = std::env::var("JCODE_CLOUD_REMOTE_BINARY") {
        args.extend(["--ssh-binary".into(), binary]);
    }
    if let Some(dir) = working_dir {
        args.extend(["--remote-working-dir".into(), dir.into()]);
    }
    args.extend(["--resume".into(), session_id.into()]);
    args
}

/// Arguments that attach a second client to a session on the SSH host this
/// client is already displaying. Everything is passed explicitly because the
/// window may be opened by the companion, which does not share our env.
pub(super) fn ssh_child_invocation_args(session_id: &str) -> Vec<String> {
    let host = crate::tui::ssh_remote_host().unwrap_or_default();
    let mut args = vec!["--fresh-spawn".to_string(), "--ssh".to_string(), host];
    for (flag, variable) in [
        ("--ssh-binary", "JCODE_SSH_BINARY"),
        ("--ssh-server-socket", "JCODE_SSH_SERVER_SOCKET"),
        ("--remote-working-dir", "JCODE_SSH_WORKING_DIR"),
    ] {
        if let Ok(value) = std::env::var(variable)
            && !value.trim().is_empty()
        {
            args.extend([flag.to_string(), value]);
        }
    }
    args.extend(["--resume".to_string(), session_id.to_string()]);
    if jcode_selfdev_types::client_selfdev_requested() {
        args.push("self-dev".to_string());
    }
    args
}

pub(super) fn ssh_child_manual_command(session_id: &str) -> String {
    command_display(
        Path::new("jcode"),
        &ssh_child_invocation_args(session_id)[1..],
    )
}

/// Open another SSH client on `session_id` in a new terminal window.
pub(super) fn spawn_ssh_child_in_new_terminal(session_id: &str) -> anyhow::Result<bool> {
    if cfg!(test) {
        // Never launch real terminal windows from unit tests.
        return Ok(false);
    }
    let cwd = std::env::var_os("JCODE_SSH_LOCAL_WORKING_DIR")
        .map(PathBuf::from)
        .filter(|path| path.is_dir())
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    let host = crate::tui::ssh_remote_host().unwrap_or_default();
    let mut command = crate::terminal_launch::TerminalCommand::new(
        launch_client_executable(),
        ssh_child_invocation_args(session_id),
    )
    .title(format!("jcode SSH {host}"))
    // Attaching must not replay a cloud handoff from this client's env.
    .spawn_env("JCODE_CLOUD_CONTINUE_MESSAGE", "")
    .spawn_env("JCODE_CLOUD_HANDOFF_PREVIEW_SESSION", "");
    if std::env::var("JCODE_SSH_LOCAL_IS_SOURCE").as_deref() != Ok("1") {
        // Our cached client cwd is inherited by the child. It was never
        // authorized as a migration source, so it must not become one there.
        command = command.spawn_env("JCODE_SSH_LOCAL_INHERITED", "1");
    }
    crate::terminal_launch::spawn_command_in_new_terminal(&command, &cwd)
}

fn command_display(program: &Path, args: &[String]) -> String {
    std::iter::once(program.to_string_lossy().to_string())
        .chain(args.iter().cloned())
        .collect::<Vec<_>>()
        .join(" ")
}

pub(super) fn build_resume_command(
    target: &ResumeTarget,
    socket: Option<&str>,
) -> (PathBuf, Vec<String>, String) {
    match target {
        ResumeTarget::JcodeSession { session_id } => {
            let exe = launch_client_executable();
            let args = resume_invocation_args(session_id, socket);
            let title = resumed_window_title(session_id);
            (exe, args, title)
        }
        ResumeTarget::ClaudeCodeSession { session_id, .. } => {
            let exe = launch_client_executable();
            let imported_id = crate::import::imported_claude_code_session_id(session_id);
            let args = resume_invocation_args(&imported_id, socket);
            let title = format!(
                "🧵 Claude Code {}",
                jcode_core::util::truncate_str(session_id, 8)
            );
            (exe, args, title)
        }
        ResumeTarget::CodexSession { session_id, .. } => {
            let exe = launch_client_executable();
            let imported_id = crate::import::imported_codex_session_id(session_id);
            let args = resume_invocation_args(&imported_id, socket);
            let title = format!("🧠 Codex {}", jcode_core::util::truncate_str(session_id, 8));
            (exe, args, title)
        }
        ResumeTarget::PiSession { session_path } => {
            let exe = launch_client_executable();
            let imported_id = crate::import::imported_pi_session_id(session_path);
            let args = resume_invocation_args(&imported_id, socket);
            let title = format!(
                "π Pi {}",
                Path::new(session_path)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("session")
            );
            (exe, args, title)
        }
        ResumeTarget::OpenCodeSession { session_id, .. } => {
            let exe = launch_client_executable();
            let imported_id = crate::import::imported_opencode_session_id(session_id);
            let args = resume_invocation_args(&imported_id, socket);
            let title = format!(
                "◌ OpenCode {}",
                jcode_core::util::truncate_str(session_id, 8)
            );
            (exe, args, title)
        }
        ResumeTarget::CursorSession { session_id, .. } => {
            let exe = launch_client_executable();
            let imported_id = crate::import::imported_cursor_session_id(session_id);
            let args = resume_invocation_args(&imported_id, socket);
            let title = format!("▮ Cursor {}", jcode_core::util::truncate_str(session_id, 8));
            (exe, args, title)
        }
    }
}

pub(super) fn resume_target_manual_command(target: &ResumeTarget, socket: Option<&str>) -> String {
    let (exe, args, _) = build_resume_command(target, socket);
    command_display(&exe, &args)
}

fn spawn_command_in_new_terminal(
    program: &Path,
    args: &[String],
    title: &str,
    cwd: &Path,
) -> anyhow::Result<bool> {
    if cfg!(test) {
        // Never launch real terminal windows from unit tests. Server-event
        // handlers (e.g. SplitResponse) call this with current_exe(), which in
        // tests is the libtest harness and would pop up a broken window.
        return Ok(false);
    }
    let mut command = crate::terminal_launch::TerminalCommand::new(program, args.to_vec())
        .title(title.to_string());
    if args.iter().any(|arg| arg == "--ssh") {
        // Resuming is an attachment, never a request to start another turn.
        command = command
            .spawn_env("JCODE_CLOUD_CONTINUE_MESSAGE", "")
            .spawn_env("JCODE_CLOUD_HANDOFF_PREVIEW_SESSION", "");
    }
    crate::terminal_launch::spawn_command_in_new_terminal(&command, cwd)
}

pub(super) fn spawn_resume_target_in_new_terminal(
    target: &ResumeTarget,
    cwd: &Path,
    socket: Option<&str>,
) -> anyhow::Result<bool> {
    let (program, args, title) = build_resume_command(target, socket);
    spawn_command_in_new_terminal(&program, &args, &title, cwd)
}

/// Build the terminal command used to spawn a brand-new jcode session.
/// Split from `spawn_fresh_session_in_new_terminal` so tests can verify the
/// invocation without launching a window.
fn build_fresh_session_command(socket: Option<&str>) -> crate::terminal_launch::TerminalCommand {
    let exe = launch_client_executable();
    let mut args = vec!["--fresh-spawn".to_string()];
    if let Some(socket) = socket.map(str::trim).filter(|s| !s.is_empty()) {
        args.push("--socket".to_string());
        args.push(socket.to_string());
    }
    crate::terminal_launch::TerminalCommand::new(&exe, args)
        .title("jcode · new session".to_string())
        .kind("new-terminal")
        .fresh_spawn()
}

/// Spawn a brand-new jcode session in a new terminal window, staying on the
/// same server socket when one is configured. Returns Ok(true) when a terminal
/// was launched, Ok(false) when no supported terminal was found.
pub(super) fn spawn_fresh_session_in_new_terminal(cwd: &Path) -> anyhow::Result<bool> {
    if cfg!(test) {
        // Never launch real terminal windows from unit tests.
        return Ok(false);
    }
    let socket = std::env::var("JCODE_SOCKET").ok();
    let command = build_fresh_session_command(socket.as_deref());
    crate::terminal_launch::spawn_command_in_new_terminal(&command, cwd)
}

fn resumed_window_title(session_id: &str) -> String {
    let session_name = crate::process_title::session_name(session_id);
    let icon = crate::id::session_icon(&session_name);
    let display_title = crate::process_title::terminal_display_title_for_id(session_id);
    let session_label = crate::process_title::terminal_session_label(&session_name, None);
    let fallback_label = if let Some(server_info) =
        crate::registry::find_server_by_socket_sync(&crate::server::socket_path())
    {
        format!("jcode/{} {}", server_info.name, session_label)
    } else {
        format!("jcode {}", session_label)
    };
    crate::process_title::terminal_window_title(
        icon,
        display_title.as_deref(),
        Some(&fallback_label),
        false,
    )
}

/// Open `session_id` in a new terminal window.
///
/// Routes through `terminal_launch` on every platform. This used to be a
/// hardcoded `Ok(false)` off Unix, which made `/judge`, `/fork`, `/review`,
/// `/transfer` and crash-restore silently print "No terminal found" on Windows
/// even though the launcher already had Windows Terminal / Alacritty / WezTerm
/// detection plus a `cmd /C start` fallback (see #715).
pub(super) fn spawn_in_new_terminal(
    exe: &Path,
    session_id: &str,
    cwd: &Path,
    socket: Option<&str>,
) -> anyhow::Result<bool> {
    let title = resumed_window_title(session_id);
    let args = resume_invocation_args(session_id, socket);
    spawn_command_in_new_terminal(exe, &args, &title, cwd)
}

#[cfg(test)]
#[path = "helpers_tests.rs"]
mod helpers_tests;

pub(super) fn copy_image_to_clipboard(media_type: &str, base64_data: &str) -> bool {
    use base64::Engine;
    use std::borrow::Cow;
    use std::io::Write;
    use std::process::Stdio;

    let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(base64_data) else {
        return false;
    };
    // `wl-copy` keeps serving the selection after this function returns. A
    // short-lived arboard owner can disappear as soon as Clipboard is dropped.
    if std::env::var("WAYLAND_DISPLAY").is_ok()
        && let Ok(mut child) = std::process::Command::new("wl-copy")
            .args(["--type", media_type])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
    {
        let wrote = child
            .stdin
            .take()
            .is_some_and(|mut stdin| stdin.write_all(&bytes).is_ok());
        if wrote && child.wait().is_ok_and(|status| status.success()) {
            return true;
        }
    }
    let Ok(decoded) = image::load_from_memory_with_format(
        &bytes,
        match media_type {
            "image/jpeg" => image::ImageFormat::Jpeg,
            "image/gif" => image::ImageFormat::Gif,
            "image/webp" => image::ImageFormat::WebP,
            _ => image::ImageFormat::Png,
        },
    ) else {
        return false;
    };
    let rgba = decoded.to_rgba8();
    let (width, height) = rgba.dimensions();
    arboard::Clipboard::new()
        .and_then(|mut clipboard| {
            clipboard.set_image(arboard::ImageData {
                width: width as usize,
                height: height as usize,
                bytes: Cow::Owned(rgba.into_raw()),
            })
        })
        .is_ok()
}

/// Extract an image URL from text that looks like an HTML img tag or a bare image URL.
/// Returns the URL if found.
pub(super) fn extract_image_url(text: &str) -> Option<String> {
    let trimmed = text.trim();

    // Check for <img src="..."> pattern (Discord web copies)
    if let Some(start) = trimmed.find("<img") {
        if let Some(src_start) = trimmed[start..].find("src=\"") {
            let url_start = start + src_start + 5;
            if let Some(url_end) = trimmed[url_start..].find('"') {
                let url = &trimmed[url_start..url_start + url_end];
                if url.starts_with("http") {
                    return Some(url.to_string());
                }
            }
        }
        if let Some(src_start) = trimmed[start..].find("src='") {
            let url_start = start + src_start + 5;
            if let Some(url_end) = trimmed[url_start..].find('\'') {
                let url = &trimmed[url_start..url_start + url_end];
                if url.starts_with("http") {
                    return Some(url.to_string());
                }
            }
        }
    }

    // Check for bare image URL
    if trimmed.starts_with("http")
        && (trimmed.contains(".png")
            || trimmed.contains(".jpg")
            || trimmed.contains(".jpeg")
            || trimmed.contains(".gif")
            || trimmed.contains(".webp"))
    {
        // Strip query params for extension check but return full URL
        return Some(trimmed.to_string());
    }

    None
}

/// Download an image from a URL and return (media_type, base64_data).
/// Uses curl for simplicity (available on all platforms).
pub(super) fn download_image_url(url: &str) -> Option<(String, String)> {
    use base64::Engine;

    let output = std::process::Command::new("curl")
        .args(["-sL", "--max-time", "10", "--max-filesize", "10000000", url])
        .output()
        .ok()?;

    if !output.status.success() || output.stdout.is_empty() {
        return None;
    }

    // Detect image type from magic bytes
    let data = &output.stdout;
    let media_type = if data.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        "image/png"
    } else if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "image/jpeg"
    } else if data.starts_with(b"GIF8") {
        "image/gif"
    } else if data.starts_with(b"RIFF") && data.len() > 12 && &data[8..12] == b"WEBP" {
        "image/webp"
    } else {
        return None;
    };

    let b64 = base64::engine::general_purpose::STANDARD.encode(data);
    Some((media_type.to_string(), b64))
}

/// Encode raw RGBA pixel data as PNG bytes.
pub(super) fn encode_rgba_as_png(width: usize, height: usize, rgba: &[u8]) -> Option<Vec<u8>> {
    use image::{ImageBuffer, RgbaImage};
    use std::io::Cursor;

    let img: RgbaImage = ImageBuffer::from_raw(width as u32, height as u32, rgba.to_vec())?;
    let mut buf = Vec::new();
    img.write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Png)
        .ok()?;
    Some(buf)
}

#[cfg(test)]
pub(super) fn gather_git_info() -> Option<GitInfo> {
    if crate::tui::is_ssh_remote() {
        return None;
    }
    GIT_INFO_CACHE
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().and_then(|(_, cached, _)| cached.clone()))
}

#[cfg(not(test))]
pub(super) fn gather_git_info() -> Option<GitInfo> {
    if crate::tui::is_ssh_remote() {
        return None;
    }

    const TTL: Duration = Duration::from_secs(5);

    // Tests never probe the live repository. The probe runs on a background
    // thread and writes its answer into this process-global cache, so the
    // first test to call this gets `None` while every later test in the same
    // binary silently inherits the developer's real branch and dirty counts.
    // That made frame assertions depend on how many tests ran before them and
    // on whether the checkout happened to be clean.
    //
    // Tests that want git data seed it explicitly with
    // `seed_git_info_cache_for_tests`, which marks the entry `refreshing` and
    // is honored by the read below.
    #[cfg(test)]
    {
        return GIT_INFO_CACHE
            .lock()
            .ok()
            .and_then(|guard| guard.as_ref().map(|(_, cached, _)| cached.clone()))
            .flatten();
    }

    #[cfg(not(test))]
    {
        if let Ok(mut guard) = GIT_INFO_CACHE.lock() {
            if let Some((ts, cached, refreshing)) = guard.as_mut() {
                if ts.elapsed() < TTL {
                    return cached.clone();
                }
                if *refreshing {
                    return cached.clone();
                }
                // The inotify watcher refreshes on change. Nothing to poll.
                if git_watch::is_healthy() {
                    *ts = std::time::Instant::now();
                    return cached.clone();
                }
                // Nothing in .git moved and the last full probe is recent:
                // extend the entry without spawning any git process.
                if let Some(root) = cached.as_ref().and_then(|info| info.repo_root.as_deref())
                    && let Ok(stamp_guard) = GIT_PROBE_STAMP.lock()
                    && let Some((probed_at, stamp)) = stamp_guard.as_ref()
                    && probed_at.elapsed() < GIT_FULL_PROBE_INTERVAL
                    && git_stamp(root).as_ref() == Some(stamp)
                {
                    *ts = std::time::Instant::now();
                    return cached.clone();
                }
                let stale = cached.clone();
                *refreshing = true;
                spawn_git_info_refresh();
                return stale;
            }

            *guard = Some((backdated_now(TTL + Duration::from_secs(1)), None, true));
            spawn_git_info_refresh();
        }
        None
    }
}

/// Run the git probe on a background thread and store the result. When the
/// value changed, wake the UI loop: the probe finishes between frames, and an
/// idle client otherwise kept showing the stale (or, at startup, empty) branch
/// and dirty counts until some unrelated event happened to redraw.
#[cfg(not(test))]
fn spawn_git_info_refresh() {
    std::thread::spawn(|| {
        run_git_info_refresh();
    });
}

/// Watcher callback: probe now, on the watcher thread (it is already
/// debounced and off the render path).
#[cfg(not(test))]
fn git_watch_changed() {
    if let Ok(mut guard) = GIT_INFO_CACHE.lock()
        && let Some((_, _, refreshing)) = guard.as_mut()
    {
        *refreshing = true;
    }
    run_git_info_refresh();
}

#[cfg(not(test))]
fn run_git_info_refresh() {
    {
        let result = gather_git_info_inner();
        if let Some(root) = result.as_ref().and_then(|info| info.repo_root.as_deref()) {
            git_watch::ensure_started(root, git_watch_changed);
        }
        if let Ok(mut stamp) = GIT_PROBE_STAMP.lock() {
            *stamp = result
                .as_ref()
                .and_then(|info| info.repo_root.as_deref())
                .and_then(git_stamp)
                .map(|s| (std::time::Instant::now(), s));
        }
        let changed = if let Ok(mut guard) = GIT_INFO_CACHE.lock() {
            let changed = guard
                .as_ref()
                .is_none_or(|(_, cached, _)| *cached != result);
            *guard = Some((std::time::Instant::now(), result, false));
            changed
        } else {
            false
        };
        if changed {
            crate::bus::Bus::global().publish(crate::bus::BusEvent::GitInfoRefreshed);
        }
    }
}

/// Start the git probe and give it up to `budget` to land before returning.
///
/// Called once just before the first frame. The probe usually finishes in a
/// couple of tens of milliseconds, so a short bounded wait lets the very first
/// frame already show the branch, dirty counts, and commits instead of
/// painting without them and growing a git section ~100ms later. If git is
/// slow (huge repo, cold cache, network filesystem) the wait times out and the
/// result arrives through the normal `GitInfoRefreshed` wake.
pub(crate) fn prime_git_info(budget: Duration) {
    #[cfg(not(test))]
    {
        if crate::tui::is_ssh_remote() {
            return;
        }
        let _ = gather_git_info();
        let deadline = std::time::Instant::now() + budget;
        loop {
            let pending = match GIT_INFO_CACHE.lock() {
                Ok(guard) => guard.as_ref().is_some_and(|(_, _, refreshing)| *refreshing),
                Err(error) => {
                    crate::logging::warn(&format!("Git warmup cache lock poisoned: {error}"));
                    false
                }
            };
            if !pending || std::time::Instant::now() >= deadline {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    #[cfg(test)]
    let _ = budget;
}

/// Fetch a session's todos plus its goal-level assessments through the same
/// stale-while-revalidate cache for consumers that need goal metadata
/// (closed feedback loop and objectives) without extra disk reads per frame.
#[cfg(test)]
pub(super) fn gather_todos_and_goals_for_session(
    session_id: Option<&str>,
) -> (Vec<TodoItem>, Vec<crate::todo::TodoGoal>) {
    if crate::tui::is_ssh_remote() {
        return (Vec::new(), Vec::new());
    }
    use std::time::Instant;

    const TTL: Duration = Duration::from_secs(1);

    let Some(session_id) = session_id else {
        return (Vec::new(), Vec::new());
    };

    fn fetch(session_id: &str) -> (Vec<TodoItem>, Vec<crate::todo::TodoGoal>) {
        (
            crate::todo::load_todos(session_id).unwrap_or_default(),
            crate::todo::load_goals(session_id).unwrap_or_default(),
        )
    }

    if let Ok(mut cache) = TODOS_CACHE.lock() {
        if let Some((ts, todos, goals, refreshing)) = cache.get_mut(session_id) {
            if ts.elapsed() < TTL {
                return (todos.clone(), goals.clone());
            }
            if *refreshing {
                return (todos.clone(), goals.clone());
            }
            let stale = (todos.clone(), goals.clone());
            *refreshing = true;
            let session_id = session_id.to_string();
            std::thread::spawn(move || {
                let (todos, goals) = fetch(&session_id);
                if let Ok(mut cache) = TODOS_CACHE.lock() {
                    cache.insert(session_id, (Instant::now(), todos, goals, false));
                }
            });
            return stale;
        }

        let session_id = session_id.to_string();
        cache.insert(
            session_id.clone(),
            (
                backdated_now(TTL + Duration::from_secs(1)),
                Vec::new(),
                Vec::new(),
                true,
            ),
        );
        std::thread::spawn(move || {
            let (todos, goals) = fetch(&session_id);
            if let Ok(mut cache) = TODOS_CACHE.lock() {
                cache.insert(session_id, (Instant::now(), todos, goals, false));
            }
        });
    }
    (Vec::new(), Vec::new())
}

pub(super) fn gather_ambient_info(ambient_enabled: bool) -> Option<AmbientWidgetData> {
    if crate::tui::is_ssh_remote() {
        // The cache and queue are laptop-local. The native protocol does not
        // currently carry remote scheduler state, so leave both widget/footer
        // absent instead of displaying unrelated local tasks.
        return None;
    }
    const TTL: Duration = Duration::from_secs(2);

    let refresh = if let Ok(mut guard) = AMBIENT_INFO_CACHE.lock() {
        if let Some((ts, cached_enabled, cached, refreshing)) = guard.as_mut() {
            if *cached_enabled == ambient_enabled && ts.elapsed() < TTL {
                return cached.clone();
            }
            if *cached_enabled == ambient_enabled && *refreshing {
                return cached.clone();
            }
            let stale = if *cached_enabled == ambient_enabled {
                cached.clone()
            } else {
                None
            };
            *refreshing = true;
            *cached_enabled = ambient_enabled;
            // Refresh after the guard is dropped: the synchronous test path
            // re-locks the cache when storing the result, so refreshing while
            // the guard is still alive would self-deadlock.
            Some((stale, false))
        } else {
            *guard = Some((
                backdated_now(TTL + Duration::from_secs(1)),
                ambient_enabled,
                None,
                true,
            ));
            Some((None, true))
        }
    } else {
        None
    };

    let (stale, _first_fill) = refresh?;
    spawn_ambient_info_refresh(ambient_enabled);
    stale
}

/// Refresh the ambient cache off the render path.
///
/// Under `cfg(test)` there is no render loop to serve, and the background
/// refresh is exactly what makes the suite parallel-unsafe: it re-reads the
/// queue under whatever `JCODE_HOME` is current when the thread runs, not when
/// it was queued, so an in-flight refresh can overwrite the cleared process
/// cache with data loaded from another test's temp home (upstream #596). The
/// synchronous tests read via `gather_ambient_info_inner` directly, so keeping
/// the refresh inline under tests is both correct and deterministic.
fn spawn_ambient_info_refresh(ambient_enabled: bool) {
    #[cfg(test)]
    {
        let result = gather_ambient_info_inner(ambient_enabled);
        if let Ok(mut guard) = AMBIENT_INFO_CACHE.lock() {
            *guard = Some((std::time::Instant::now(), ambient_enabled, result, false));
        }
    }
    #[cfg(not(test))]
    {
        std::thread::spawn(move || {
            let result = gather_ambient_info_inner(ambient_enabled);
            if let Ok(mut guard) = AMBIENT_INFO_CACHE.lock() {
                *guard = Some((std::time::Instant::now(), ambient_enabled, result, false));
            }
        });
    }
}

#[cfg(test)]
pub(crate) fn clear_ambient_info_cache_for_tests() {
    if let Ok(mut guard) = AMBIENT_INFO_CACHE.lock() {
        *guard = None;
    }
}

pub(crate) fn format_countdown_until(target: chrono::DateTime<chrono::Utc>) -> String {
    let secs = (target - chrono::Utc::now()).num_seconds().max(0);
    match secs {
        0..=59 => format!("in {}s", secs),
        60..=3599 => {
            let mins = secs / 60;
            let rem = secs % 60;
            if rem == 0 {
                format!("in {}m", mins)
            } else {
                format!("in {}m {}s", mins, rem)
            }
        }
        _ => {
            let hours = secs / 3600;
            let mins = (secs % 3600) / 60;
            if mins == 0 {
                format!("in {}h", hours)
            } else {
                format!("in {}h {}m", hours, mins)
            }
        }
    }
}

#[cfg_attr(test, allow(dead_code))]
pub(crate) fn gather_git_info_inner() -> Option<GitInfo> {
    gather_git_info_in(None)
}

/// Parse `git log --shortstat --format=%x1e%h%x1f%ct%x1f%s`. The first
/// `ahead` commits are the ones not yet on the upstream.
pub(crate) fn parse_recent_commits(
    text: &str,
    ahead: usize,
) -> Vec<crate::tui::info_widget::RecentCommit> {
    text.split('\x1e')
        .filter(|record| !record.trim().is_empty())
        .enumerate()
        .filter_map(|(index, record)| {
            let mut lines = record.lines();
            let mut fields = lines.next()?.splitn(3, '\x1f');
            let hash = fields.next()?.trim().to_string();
            let timestamp = match fields.next()?.trim().parse() {
                Ok(timestamp) => timestamp,
                Err(error) => {
                    crate::logging::debug(&format!("Cannot parse git commit timestamp: {error}"));
                    return None;
                }
            };
            let subject = fields.next().unwrap_or("").trim().to_string();
            let (mut added, mut removed) = (None, None);
            // " 3 files changed, 12 insertions(+), 4 deletions(-)"
            for part in lines.flat_map(|l| l.split(',')) {
                let part = part.trim();
                let n = part.split_whitespace().next().and_then(|n| n.parse().ok());
                if part.contains("insertion") {
                    added = n;
                } else if part.contains("deletion") {
                    removed = n;
                } else if part.contains("changed") {
                    added = added.or(Some(0));
                    removed = removed.or(Some(0));
                }
            }
            Some(crate::tui::info_widget::RecentCommit {
                hash,
                subject,
                timestamp,
                unpushed: index < ahead,
                added,
                removed,
            })
        })
        .collect()
}

/// Parse `git diff --numstat` into path -> (added, removed). Binary files
/// report `-` and map to `None` counts.
pub(crate) fn parse_numstat(
    text: &str,
) -> std::collections::HashMap<String, (Option<usize>, Option<usize>)> {
    let mut out = std::collections::HashMap::new();
    for line in text.lines() {
        let mut parts = line.splitn(3, '\t');
        let (Some(a), Some(r), Some(path)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        // Renames: `old => new` or `dir/{old => new}/file`.
        let path = if let (Some(open), Some(close)) = (path.find('{'), path.find('}')) {
            let inner = &path[open + 1..close];
            let new = inner.rsplit(" => ").next().unwrap_or(inner);
            format!("{}{}{}", &path[..open], new, &path[close + 1..]).replace("//", "/")
        } else {
            path.rsplit(" => ").next().unwrap_or(path).to_string()
        };
        out.insert(path, (a.parse().ok(), r.parse().ok()));
    }
    out
}

/// Collapse a porcelain `XY` pair into the single letter the Changes widget
/// shows. Conflicts win, then the worktree side (what the user is editing),
/// then the index side.
pub(crate) fn porcelain_status_letter(index: u8, worktree: u8) -> char {
    if index == b'?' {
        return '?';
    }
    if index == b'U' || worktree == b'U' || (index == b'A' && worktree == b'A') {
        return 'U';
    }
    let pick = if worktree != b' ' { worktree } else { index };
    match pick {
        b'M' | b'T' => 'M',
        b'A' => 'A',
        b'D' => 'D',
        b'R' | b'C' => 'R',
        _ => 'M',
    }
}

#[cfg(test)]
#[path = "helpers/account_notice_tests.rs"]
mod account_notice_tests;

#[cfg(test)]
mod git_stamp_tests {
    #[test]
    fn stamp_changes_when_head_ref_moves() {
        let dir = tempfile::tempdir().unwrap();
        let git = dir.path().join(".git");
        std::fs::create_dir_all(git.join("refs/heads")).unwrap();
        std::fs::write(git.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::write(git.join("refs/heads/main"), "a\n").unwrap();
        let before = super::git_stamp(dir.path()).unwrap();
        assert_eq!(super::git_stamp(dir.path()).unwrap(), before);
        std::fs::write(git.join("refs/heads/main"), "bb\n").unwrap();
        assert_ne!(super::git_stamp(dir.path()).unwrap(), before);
        assert!(super::git_stamp(&dir.path().join("missing")).is_none());
    }
}
