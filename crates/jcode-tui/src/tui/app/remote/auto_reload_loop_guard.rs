//! Auto-reload loop guard: shared env-based state that stops the client
//! from force-reloading the session in a tight loop when the server keeps
//! failing to come up.

const AUTO_RELOAD_LOOP_GUARD_ENV: &str = "JCODE_AUTO_RELOAD_LOOP_GUARD";
const AUTO_RELOAD_LOOP_WINDOW_SECS: u64 = 30;
const AUTO_RELOAD_LOOP_MAX_ATTEMPTS: u32 = 2;

fn parse_auto_reload_loop_guard(raw: &str) -> Option<(u64, u32)> {
    let (ts, count) = raw.split_once(':')?;
    Some((ts.parse().ok()?, count.parse().ok()?))
}

pub(super) fn format_next_auto_reload_loop_guard(prev: Option<&str>, now_secs: u64) -> String {
    let next_count = prev
        .and_then(parse_auto_reload_loop_guard)
        .filter(|(ts, _)| now_secs.saturating_sub(*ts) <= AUTO_RELOAD_LOOP_WINDOW_SECS)
        .map(|(_, count)| count.saturating_add(1))
        .unwrap_or(1);
    format!("{now_secs}:{next_count}")
}

pub(super) fn auto_reload_loop_guard_tripped_at(raw: Option<&str>, now_secs: u64) -> bool {
    raw.and_then(parse_auto_reload_loop_guard)
        .is_some_and(|(ts, count)| {
            now_secs.saturating_sub(ts) <= AUTO_RELOAD_LOOP_WINDOW_SECS
                && count >= AUTO_RELOAD_LOOP_MAX_ATTEMPTS
        })
}

pub(in crate::tui::app) fn record_auto_reload_attempt() {
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let prev = std::env::var(AUTO_RELOAD_LOOP_GUARD_ENV).ok();
    let next = format_next_auto_reload_loop_guard(prev.as_deref(), now_secs);
    crate::env::set_var(AUTO_RELOAD_LOOP_GUARD_ENV, next);
}

pub(in crate::tui::app::remote) fn auto_reload_loop_guard_tripped() -> bool {
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let raw = std::env::var(AUTO_RELOAD_LOOP_GUARD_ENV).ok();
    auto_reload_loop_guard_tripped_at(raw.as_deref(), now_secs)
}

pub(in crate::tui::app::remote) fn clear_auto_reload_loop_guard() {
    if std::env::var_os(AUTO_RELOAD_LOOP_GUARD_ENV).is_some() {
        crate::env::remove_var(AUTO_RELOAD_LOOP_GUARD_ENV);
    }
}
