use super::{SESSION_STATE, sanitize_telemetry_label};
use crate::storage;
use chrono::{DateTime, Datelike, Timelike, Utc};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

pub(super) fn telemetry_id_path() -> Option<PathBuf> {
    storage::jcode_dir().ok().map(|d| d.join("telemetry_id"))
}

pub(super) fn install_recorded_path() -> Option<PathBuf> {
    storage::jcode_dir()
        .ok()
        .map(|d| d.join("telemetry_install_sent"))
}

pub(super) fn version_recorded_path() -> Option<PathBuf> {
    storage::jcode_dir()
        .ok()
        .map(|d| d.join("telemetry_version_sent"))
}

pub(super) fn telemetry_state_path(name: &str) -> Option<PathBuf> {
    storage::jcode_dir().ok().map(|d| d.join(name))
}

pub(super) fn milestone_recorded_path(id: &str, key: &str) -> Option<PathBuf> {
    telemetry_state_path(&format!(
        "telemetry_milestone_{}_{}",
        sanitize_telemetry_label(key),
        id
    ))
}

pub(super) fn onboarding_step_milestone_key(
    step: &str,
    auth_provider: Option<&str>,
    auth_method: Option<&str>,
) -> String {
    fn normalize_part(value: &str) -> String {
        let sanitized = sanitize_telemetry_label(value);
        let collapsed = sanitized
            .split_whitespace()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("_");
        collapsed.to_ascii_lowercase()
    }

    let mut parts = vec![normalize_part(step)];
    if let Some(provider) = auth_provider {
        let provider = normalize_part(provider);
        if !provider.is_empty() {
            parts.push(provider);
        }
    }
    if let Some(method) = auth_method {
        let method = normalize_part(method);
        if !method.is_empty() {
            parts.push(method);
        }
    }
    parts.join("_")
}

pub(super) fn active_days_path(id: &str) -> Option<PathBuf> {
    telemetry_state_path(&format!("telemetry_active_days_{}.txt", id))
}

pub(super) fn session_starts_path(id: &str) -> Option<PathBuf> {
    telemetry_state_path(&format!("telemetry_session_starts_{}.txt", id))
}

pub(super) fn active_sessions_dir() -> Option<PathBuf> {
    telemetry_state_path("telemetry_active_sessions")
}

pub(super) fn active_session_file(session_id: &str) -> Option<PathBuf> {
    active_sessions_dir().map(|dir| dir.join(format!("{}.active", session_id)))
}

pub(super) fn write_private_file(path: &PathBuf, value: &str) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, value);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
}

pub(super) fn utc_hour(timestamp: DateTime<Utc>) -> u32 {
    timestamp.hour()
}

pub(super) fn utc_weekday(timestamp: DateTime<Utc>) -> u32 {
    timestamp.weekday().num_days_from_monday()
}

pub(super) fn write_private_dir_file(path: &PathBuf, value: &str) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    write_private_file(path, value);
}

pub(super) fn read_epoch_lines(path: &PathBuf) -> Vec<i64> {
    std::fs::read_to_string(path)
        .ok()
        .into_iter()
        .flat_map(|text| {
            text.lines()
                .map(str::trim)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .filter_map(|line| line.parse::<i64>().ok())
        .collect()
}

pub(super) fn update_session_start_history(
    id: &str,
    started_at_utc: DateTime<Utc>,
) -> (Option<u64>, u32, u32) {
    let Some(path) = session_starts_path(id) else {
        return (None, 0, 0);
    };
    let now = started_at_utc.timestamp();
    let cutoff_30d = now - 30 * 24 * 60 * 60;
    let mut starts = read_epoch_lines(&path)
        .into_iter()
        .filter(|value| *value >= cutoff_30d)
        .collect::<Vec<_>>();
    starts.sort_unstable();
    let previous = starts.last().copied();
    starts.push(now);
    let rendered = starts
        .iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    write_private_dir_file(&path, &rendered);
    let sessions_started_24h = starts
        .iter()
        .filter(|value| now.saturating_sub(**value) < 24 * 60 * 60)
        .count()
        .min(u32::MAX as usize) as u32;
    let sessions_started_7d = starts
        .iter()
        .filter(|value| now.saturating_sub(**value) < 7 * 24 * 60 * 60)
        .count()
        .min(u32::MAX as usize) as u32;
    let previous_session_gap_secs = previous
        .and_then(|value| now.checked_sub(value))
        .map(|value| value.min(u64::MAX as i64) as u64);
    (
        previous_session_gap_secs,
        sessions_started_24h,
        sessions_started_7d,
    )
}

pub(super) fn prune_active_session_files(dir: &PathBuf) -> u32 {
    let _ = std::fs::create_dir_all(dir);
    let now = SystemTime::now();
    let max_age = Duration::from_secs(24 * 60 * 60);
    let mut count = 0u32;
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return 0,
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let fresh = entry
            .metadata()
            .ok()
            .and_then(|meta| meta.modified().ok())
            .and_then(|modified| now.duration_since(modified).ok())
            .map(|age| age <= max_age)
            .unwrap_or(false);
        if fresh {
            count = count.saturating_add(1);
        } else {
            let _ = std::fs::remove_file(path);
        }
    }
    count
}

pub(super) fn register_active_session(session_id: &str) -> (u32, u32) {
    let Some(dir) = active_sessions_dir() else {
        return (0, 0);
    };
    let existing = prune_active_session_files(&dir);
    if let Some(path) = active_session_file(session_id) {
        write_private_dir_file(&path, "1");
    }
    (existing.saturating_add(1), existing)
}

pub(super) fn observe_active_sessions() -> u32 {
    active_sessions_dir()
        .map(|dir| prune_active_session_files(&dir))
        .unwrap_or(0)
}

pub(super) fn unregister_active_session(session_id: &str) {
    if let Some(path) = active_session_file(session_id) {
        let _ = std::fs::remove_file(path);
    }
}

pub(super) fn get_or_create_id() -> Option<String> {
    let path = telemetry_id_path()?;
    if let Ok(id) = std::fs::read_to_string(&path) {
        let id = id.trim().to_string();
        if !id.is_empty() {
            return Some(id);
        }
    }
    let id = uuid::Uuid::new_v4().to_string();
    write_private_file(&path, &id);
    Some(id)
}

pub(super) fn is_first_run() -> bool {
    telemetry_id_path().map(|p| !p.exists()).unwrap_or(false)
}

pub(super) fn version() -> String {
    jcode_build_meta::PKG_VERSION.to_string()
}

pub(super) fn install_recorded_for_id(id: &str) -> bool {
    install_recorded_path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .map(|stored| stored.trim() == id)
        .unwrap_or(false)
}

pub(super) fn mark_install_recorded(id: &str) {
    if let Some(path) = install_recorded_path() {
        write_private_file(&path, id);
    }
}

pub(super) fn previously_recorded_version() -> Option<String> {
    version_recorded_path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub(super) fn mark_current_version_recorded() {
    if let Some(path) = version_recorded_path() {
        write_private_file(&path, &version());
    }
}

pub(super) fn new_event_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

pub(super) fn is_jcode_repo_dir(dir: &Path) -> bool {
    let cargo_toml = dir.join("Cargo.toml");
    if !cargo_toml.exists() || !dir.join(".git").exists() {
        return false;
    }

    std::fs::read_to_string(cargo_toml)
        .map(|content| content.contains("name = \"jcode\""))
        .unwrap_or(false)
}

fn find_jcode_repo_in_ancestors(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|dir| is_jcode_repo_dir(dir))
        .map(Path::to_path_buf)
}

fn telemetry_jcode_repo_dir() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("JCODE_REPO_DIR") {
        let path = PathBuf::from(path);
        if is_jcode_repo_dir(&path) {
            return Some(path);
        }
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    if let Some(repo) = find_jcode_repo_in_ancestors(&manifest_dir) {
        return Some(repo);
    }

    if let Ok(exe) = std::env::current_exe()
        && let Some(repo) = exe
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
            .filter(|dir| is_jcode_repo_dir(dir))
    {
        return Some(repo.to_path_buf());
    }

    std::env::current_dir()
        .ok()
        .and_then(|cwd| find_jcode_repo_in_ancestors(&cwd))
}

pub(super) fn build_channel() -> String {
    if std::env::var(jcode_selfdev_types::CLIENT_SELFDEV_ENV).is_ok() {
        return "selfdev".to_string();
    }
    if let Ok(exe) = std::env::current_exe() {
        let path = exe.to_string_lossy();
        if path.contains("/target/debug/") || path.contains("\\target\\debug\\") {
            return "debug".to_string();
        }
        if path.contains("/target/release/") || path.contains("\\target\\release\\") {
            return "local_build".to_string();
        }
    }
    if telemetry_jcode_repo_dir().is_some() {
        return "git_checkout".to_string();
    }
    "release".to_string()
}

pub(super) fn is_git_checkout() -> bool {
    telemetry_jcode_repo_dir().is_some()
}

pub(super) fn is_ci() -> bool {
    [
        "CI",
        "GITHUB_ACTIONS",
        "BUILDKITE",
        "JENKINS_URL",
        "GITLAB_CI",
        "CIRCLECI",
    ]
    .iter()
    .any(|key| std::env::var(key).is_ok())
}

pub(super) fn ran_from_cargo() -> bool {
    std::env::var("CARGO").is_ok() || std::env::var("CARGO_MANIFEST_DIR").is_ok()
}

pub(super) fn install_anchor_time(id: &str) -> Option<SystemTime> {
    install_recorded_path()
        .filter(|path| install_recorded_for_id(id) && path.exists())
        .and_then(|path| std::fs::metadata(path).ok())
        .and_then(|meta| meta.modified().ok())
        .or_else(|| {
            telemetry_id_path()
                .and_then(|path| std::fs::metadata(path).ok())
                .and_then(|meta| meta.modified().ok())
        })
}

pub(super) fn elapsed_since_install_ms(id: &str) -> Option<u64> {
    let anchor = install_anchor_time(id)?;
    let elapsed = SystemTime::now().duration_since(anchor).ok()?;
    Some(elapsed.as_millis().min(u128::from(u64::MAX)) as u64)
}

pub(super) fn days_since_install(id: &str) -> Option<u32> {
    let anchor = install_anchor_time(id)?;
    let elapsed = SystemTime::now().duration_since(anchor).ok()?;
    Some((elapsed.as_secs() / 86_400).min(u64::from(u32::MAX)) as u32)
}

pub(super) fn milestone_recorded(id: &str, step: &str) -> bool {
    milestone_recorded_path(id, step)
        .map(|path| path.exists())
        .unwrap_or(false)
}

pub(super) fn mark_milestone_recorded(id: &str, step: &str) {
    if let Some(path) = milestone_recorded_path(id, step) {
        write_private_file(&path, "1");
    }
}

pub(super) fn current_session_id() -> Option<String> {
    SESSION_STATE
        .lock()
        .map(|state| state.as_ref().map(|s| s.session_id.clone()))
        .ok()
        .flatten()
}
