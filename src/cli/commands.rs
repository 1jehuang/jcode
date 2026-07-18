#![cfg_attr(test, allow(clippy::await_holding_lock))]

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::net::ToSocketAddrs;
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, Stdio};

use crate::{browser, gateway, memory, session, storage, tui};

use super::terminal::init_tui_runtime;

mod menubar;
mod provider_setup;
mod report_info;
mod restart;

pub(crate) use super::auth_test::run_post_login_validation;
#[cfg(test)]
pub(crate) use super::auth_test::{
    AuthTestChoicePlan, AuthTestTarget, ResolvedAuthTestTarget, auth_test_choice_plan,
    auth_test_error_is_retryable, configured_auth_test_targets, resolve_auth_test_targets,
};
pub use super::auth_test::{
    run_auth_test_command, run_auth_test_context_audit_command, run_auth_test_coverage_command,
};
pub use menubar::{ensure_menubar_helper_running, run_menubar_command};
pub(crate) use provider_setup::{ProviderAddOptions, run_provider_add_command};
pub use restart::{
    maybe_run_pending_restart_restore_on_startup, run_restart_clear_command,
    run_restart_restore_command, run_restart_save_command, run_restart_status_command,
};

pub enum AmbientSubcommand {
    Status,
    Log,
    Trigger,
    Stop,
    RunVisible,
}

pub enum CloudSubcommand {
    Sessions(CloudSessionsSubcommand),
}

pub enum CloudSessionsSubcommand {
    Configure {
        api_base: Option<String>,
        api_token: Option<String>,
        api_token_env: Option<String>,
        api_token_id: Option<String>,
        user_id: Option<String>,
        helper: Option<String>,
        clear: bool,
    },
    Status {
        json: bool,
    },
    Upload {
        session_file: String,
        raw: bool,
        user_id: String,
        profile: Option<String>,
        region: Option<String>,
        helper: Option<String>,
    },
    UploadLatest {
        sessions_dir: String,
        raw: bool,
        user_id: String,
        profile: Option<String>,
        region: Option<String>,
        helper: Option<String>,
    },
    Sync {
        sessions_dir: Option<String>,
        since_days: Option<u64>,
        all: bool,
        max: usize,
        min_interval_mins: Option<u64>,
        raw: bool,
        dry_run: bool,
        force: bool,
        json: bool,
        user_id: String,
        profile: Option<String>,
        region: Option<String>,
        helper: Option<String>,
    },
    List {
        limit: usize,
        json: bool,
        user_id: String,
        profile: Option<String>,
        region: Option<String>,
        helper: Option<String>,
    },
    Verify {
        session_id: String,
        user_id: String,
        profile: Option<String>,
        region: Option<String>,
        helper: Option<String>,
    },
    Dashboard {
        limit: usize,
        output: Option<String>,
        open: bool,
        with_view: bool,
        user_id: String,
        profile: Option<String>,
        region: Option<String>,
        helper: Option<String>,
    },
    View {
        session_id: String,
        format: String,
        output: Option<String>,
        open: bool,
        user_id: String,
        profile: Option<String>,
        region: Option<String>,
        helper: Option<String>,
    },
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct CloudSessionsConfig {
    api_base: Option<String>,
    api_token: Option<String>,
    api_token_id: Option<String>,
    helper: Option<String>,
    user_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct CloudSessionsConfigStatus {
    path: String,
    exists: bool,
    api_base: Option<String>,
    api_token_configured: bool,
    api_token_id: Option<String>,
    helper: Option<String>,
    user_id: Option<String>,
}

pub fn run_cloud_command(cmd: CloudSubcommand) -> Result<()> {
    match cmd {
        CloudSubcommand::Sessions(action) => run_cloud_sessions_command(action),
    }
}

fn run_cloud_sessions_command(action: CloudSessionsSubcommand) -> Result<()> {
    match action {
        CloudSessionsSubcommand::Configure {
            api_base,
            api_token,
            api_token_env,
            api_token_id,
            user_id,
            helper,
            clear,
        } => run_cloud_sessions_configure(
            api_base,
            api_token,
            api_token_env,
            api_token_id,
            user_id,
            helper,
            clear,
        ),
        CloudSessionsSubcommand::Status { json } => run_cloud_sessions_status(json),
        CloudSessionsSubcommand::Dashboard {
            limit,
            output,
            open,
            with_view,
            user_id,
            profile,
            region,
            helper,
        } => run_cloud_sessions_dashboard(CloudSessionsDashboardRequest {
            limit,
            output,
            open,
            with_view,
            user_id,
            profile,
            region,
            helper,
        }),
        CloudSessionsSubcommand::Sync {
            sessions_dir,
            since_days,
            all,
            max,
            min_interval_mins,
            raw,
            dry_run,
            force,
            json,
            user_id,
            profile,
            region,
            helper,
        } => run_cloud_sessions_sync(CloudSessionsSyncRequest {
            sessions_dir,
            since_days,
            all,
            max,
            min_interval_mins,
            raw,
            dry_run,
            force,
            json,
            user_id,
            profile,
            region,
            helper,
        }),
        other => run_cloud_sessions_helper_command(other),
    }
}

fn run_cloud_sessions_helper_command(action: CloudSessionsSubcommand) -> Result<()> {
    let config = load_cloud_sessions_config()?.unwrap_or_default();
    let helper_override = cloud_sessions_helper_override(&action).or_else(|| config.helper.clone());
    let helper = resolve_jade_sessions_helper(helper_override.as_deref())?;
    let helper_env = cloud_sessions_helper_env(&config);
    let args = build_jade_sessions_args_with_config(action, &config);
    let mut command = ProcessCommand::new(&helper);
    command
        .args(&args)
        .envs(helper_env)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    let status = command
        .status()
        .map_err(|err| anyhow::anyhow!("failed to run {}: {err}", helper.display()))?;

    if !status.success() {
        anyhow::bail!("{} exited with status {status}", helper.display());
    }
    Ok(())
}

fn cloud_sessions_config_path() -> Result<PathBuf> {
    Ok(crate::storage::jcode_dir()?.join("cloud_sessions.json"))
}

fn load_cloud_sessions_config() -> Result<Option<CloudSessionsConfig>> {
    let path = cloud_sessions_config_path()?;
    if !path.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(&path)
        .map_err(|err| anyhow::anyhow!("failed to read {}: {err}", path.display()))?;
    let config = serde_json::from_str(&content)
        .map_err(|err| anyhow::anyhow!("failed to parse {}: {err}", path.display()))?;
    Ok(Some(config))
}

fn save_cloud_sessions_config(config: &CloudSessionsConfig) -> Result<PathBuf> {
    let path = cloud_sessions_config_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = serde_json::to_vec_pretty(config)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&path)?;
        file.write_all(&content)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(&path, &content)?;
    }
    Ok(path)
}

fn run_cloud_sessions_configure(
    api_base: Option<String>,
    api_token: Option<String>,
    api_token_env: Option<String>,
    api_token_id: Option<String>,
    user_id: Option<String>,
    helper: Option<String>,
    clear: bool,
) -> Result<()> {
    let path = cloud_sessions_config_path()?;
    if clear {
        match std::fs::remove_file(&path) {
            Ok(()) => println!("Removed Jade cloud sessions config at {}", path.display()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                println!("No Jade cloud sessions config found at {}", path.display());
            }
            Err(err) => return Err(err.into()),
        }
        return Ok(());
    }

    if api_base.is_none()
        && api_token.is_none()
        && api_token_env.is_none()
        && api_token_id.is_none()
        && user_id.is_none()
        && helper.is_none()
    {
        anyhow::bail!(
            "nothing to configure; pass --api-base, --api-token/--api-token-env, --api-token-id, --user-id, --helper, or --clear"
        );
    }

    let mut config = load_cloud_sessions_config()?.unwrap_or_default();
    if let Some(value) = non_empty(api_base) {
        config.api_base = Some(value);
    }
    if let Some(value) = non_empty(api_token) {
        config.api_token = Some(value);
    }
    if let Some(var) = non_empty(api_token_env) {
        let value = std::env::var(&var)
            .map_err(|err| anyhow::anyhow!("failed to read {var} for --api-token-env: {err}"))?;
        let value = non_empty(Some(value))
            .ok_or_else(|| anyhow::anyhow!("{var} for --api-token-env was empty"))?;
        config.api_token = Some(value);
    }
    if let Some(value) = non_empty(api_token_id) {
        config.api_token_id = Some(value);
    }
    if let Some(value) = non_empty(user_id) {
        config.user_id = Some(value);
    }
    if let Some(value) = non_empty(helper) {
        config.helper = Some(value);
    }

    let path = save_cloud_sessions_config(&config)?;
    println!("Saved Jade cloud sessions config to {}", path.display());
    println!("api_base: {}", configured_label(config.api_base.as_deref()));
    println!(
        "api_token: {}",
        if config.api_token.is_some() {
            "configured"
        } else {
            "not configured"
        }
    );
    println!(
        "api_token_id: {}",
        configured_label(config.api_token_id.as_deref())
    );
    println!("user_id: {}", configured_label(config.user_id.as_deref()));
    println!("helper: {}", configured_label(config.helper.as_deref()));
    Ok(())
}

fn run_cloud_sessions_status(json: bool) -> Result<()> {
    let path = cloud_sessions_config_path()?;
    let config = load_cloud_sessions_config()?.unwrap_or_default();
    let status = CloudSessionsConfigStatus {
        path: path.display().to_string(),
        exists: path.exists(),
        api_base: config.api_base,
        api_token_configured: config.api_token.is_some(),
        api_token_id: config.api_token_id,
        helper: config.helper,
        user_id: config.user_id,
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&status)?);
    } else {
        println!("Jade cloud sessions config: {}", status.path);
        println!("exists: {}", status.exists);
        println!("api_base: {}", configured_label(status.api_base.as_deref()));
        println!(
            "api_token: {}",
            if status.api_token_configured {
                "configured"
            } else {
                "not configured"
            }
        );
        println!(
            "api_token_id: {}",
            configured_label(status.api_token_id.as_deref())
        );
        println!("user_id: {}", configured_label(status.user_id.as_deref()));
        println!("helper: {}", configured_label(status.helper.as_deref()));
    }
    Ok(())
}

fn non_empty(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

struct CloudSessionsSyncRequest {
    sessions_dir: Option<String>,
    since_days: Option<u64>,
    all: bool,
    max: usize,
    min_interval_mins: Option<u64>,
    raw: bool,
    dry_run: bool,
    force: bool,
    json: bool,
    user_id: String,
    profile: Option<String>,
    region: Option<String>,
    helper: Option<String>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct CloudSessionsSyncState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_sync_at: Option<String>,
    #[serde(default)]
    sessions: std::collections::BTreeMap<String, CloudSessionsSyncRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CloudSessionsSyncRecord {
    sha256: String,
    size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    modified_unix: Option<i64>,
    uploaded_at: String,
}

#[derive(Debug, Serialize)]
struct CloudSessionsSyncEntry {
    session_id: String,
    path: String,
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct CloudSessionsSyncReport {
    sessions_dir: String,
    dry_run: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    throttled: bool,
    scanned: usize,
    uploaded: usize,
    skipped_unchanged: usize,
    failed: usize,
    reached_max: bool,
    entries: Vec<CloudSessionsSyncEntry>,
}

struct SyncCandidate {
    session_id: String,
    path: PathBuf,
    size: u64,
    modified_unix: Option<i64>,
}

fn cloud_sessions_sync_state_path() -> Result<PathBuf> {
    Ok(crate::storage::jcode_dir()?.join("cloud_sessions_sync.json"))
}

fn load_cloud_sessions_sync_state() -> Result<CloudSessionsSyncState> {
    let path = cloud_sessions_sync_state_path()?;
    if !path.exists() {
        return Ok(CloudSessionsSyncState::default());
    }
    let content = std::fs::read_to_string(&path)
        .map_err(|err| anyhow::anyhow!("failed to read {}: {err}", path.display()))?;
    serde_json::from_str(&content)
        .map_err(|err| anyhow::anyhow!("failed to parse {}: {err}", path.display()))
}

fn save_cloud_sessions_sync_state(state: &CloudSessionsSyncState) -> Result<PathBuf> {
    let path = cloud_sessions_sync_state_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = serde_json::to_vec_pretty(state)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&path)?;
        file.write_all(&content)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(&path, &content)?;
    }
    Ok(path)
}

fn resolve_sync_sessions_dir(override_path: Option<&str>) -> Result<PathBuf> {
    if let Some(path) = override_path.map(str::trim).filter(|path| !path.is_empty()) {
        return Ok(expand_home_path(path));
    }
    Ok(crate::storage::jcode_dir()?.join("sessions"))
}

fn expand_home_path(path: &str) -> PathBuf {
    if let Some(stripped) = path.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(stripped);
    }
    if path == "~"
        && let Some(home) = dirs::home_dir()
    {
        return home;
    }
    PathBuf::from(path)
}

fn is_syncable_session_stem(stem: &str) -> bool {
    (stem.starts_with("session_") || stem.starts_with("imported_")) && !stem.ends_with(".journal")
}

fn sha256_file(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    let mut file = std::fs::File::open(path)
        .map_err(|err| anyhow::anyhow!("failed to open {}: {err}", path.display()))?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)
        .map_err(|err| anyhow::anyhow!("failed to hash {}: {err}", path.display()))?;
    Ok(hex::encode(hasher.finalize()))
}

fn collect_sync_candidates(dir: &Path) -> Result<Vec<SyncCandidate>> {
    let mut candidates = Vec::new();
    if !dir.exists() {
        anyhow::bail!("sessions directory not found: {}", dir.display());
    }
    for entry in std::fs::read_dir(dir)
        .map_err(|err| anyhow::anyhow!("failed to read {}: {err}", dir.display()))?
        .flatten()
    {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        if !is_syncable_session_stem(stem) {
            continue;
        }
        let metadata = match entry.metadata() {
            Ok(metadata) if metadata.is_file() => metadata,
            _ => continue,
        };
        let modified_unix = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|dur| dur.as_secs() as i64);
        candidates.push(SyncCandidate {
            session_id: stem.to_string(),
            path,
            size: metadata.len(),
            modified_unix,
        });
    }
    Ok(candidates)
}

fn run_jade_upload(
    helper: &Path,
    helper_env: &[(&'static str, String)],
    file: &Path,
    user_id: &str,
    profile: Option<&str>,
    region: Option<&str>,
    raw: bool,
) -> Result<()> {
    let mut args = vec!["upload".to_string()];
    append_common_jade_args(
        &mut args,
        user_id.to_string(),
        profile.map(ToOwned::to_owned),
        region.map(ToOwned::to_owned),
    );
    if raw {
        args.push("--raw".to_string());
    }
    args.push(file.display().to_string());

    let output = ProcessCommand::new(helper)
        .args(&args)
        .envs(helper_env.iter().cloned())
        .stdin(Stdio::null())
        .output()
        .map_err(|err| anyhow::anyhow!("failed to run {}: {err}", helper.display()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.trim();
        let detail = if detail.is_empty() {
            format!("exited with status {}", output.status)
        } else {
            detail.lines().last().unwrap_or(detail).to_string()
        };
        anyhow::bail!(detail);
    }
    Ok(())
}

fn run_cloud_sessions_sync(request: CloudSessionsSyncRequest) -> Result<()> {
    let config = load_cloud_sessions_config()?.unwrap_or_default();
    let helper_override = request.helper.clone().or_else(|| config.helper.clone());
    let user_id = config_or_default_user_id(request.user_id.clone(), &config);
    let sessions_dir = resolve_sync_sessions_dir(request.sessions_dir.as_deref())?;
    let mut state = load_cloud_sessions_sync_state()?;

    // Self-throttle so the command is safe to call from cron/systemd timers without
    // re-uploading or even rescanning more often than requested.
    if !request.force
        && !request.dry_run
        && let Some(min_interval) = request.min_interval_mins
        && min_interval > 0
        && let Some(last) = state
            .last_sync_at
            .as_deref()
            .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
    {
        let elapsed_mins = (chrono::Utc::now() - last.with_timezone(&chrono::Utc)).num_minutes();
        if elapsed_mins < min_interval as i64 {
            let report = CloudSessionsSyncReport {
                sessions_dir: sessions_dir.display().to_string(),
                dry_run: request.dry_run,
                throttled: true,
                scanned: 0,
                uploaded: 0,
                skipped_unchanged: 0,
                failed: 0,
                reached_max: false,
                entries: Vec::new(),
            };
            if request.json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!(
                    "Jade cloud sessions sync skipped: last sync {elapsed_mins}m ago (< --min-interval-mins {min_interval})"
                );
            }
            return Ok(());
        }
    }

    let helper = resolve_jade_sessions_helper(helper_override.as_deref())?;
    let helper_env = cloud_sessions_helper_env(&config);
    let mut candidates = collect_sync_candidates(&sessions_dir)?;

    if !request.all {
        let since_days = request.since_days.unwrap_or(7);
        let cutoff = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|dur| dur.as_secs() as i64)
            .unwrap_or(0)
            - (since_days as i64) * 86_400;
        candidates.retain(|candidate| candidate.modified_unix.map(|m| m >= cutoff).unwrap_or(true));
    }

    // Newest first so --max keeps the most recent sessions.
    candidates.sort_by(|a, b| b.modified_unix.cmp(&a.modified_unix));

    let mut report = CloudSessionsSyncReport {
        sessions_dir: sessions_dir.display().to_string(),
        dry_run: request.dry_run,
        throttled: false,
        scanned: 0,
        uploaded: 0,
        skipped_unchanged: 0,
        failed: 0,
        reached_max: false,
        entries: Vec::new(),
    };
    let mut state_dirty = false;

    for candidate in candidates {
        if report.uploaded + report.failed >= request.max {
            report.reached_max = true;
            break;
        }
        report.scanned += 1;
        let sha = match sha256_file(&candidate.path) {
            Ok(sha) => sha,
            Err(err) => {
                report.failed += 1;
                report.entries.push(CloudSessionsSyncEntry {
                    session_id: candidate.session_id,
                    path: candidate.path.display().to_string(),
                    status: "failed",
                    error: Some(err.to_string()),
                });
                continue;
            }
        };

        if !request.force
            && let Some(record) = state.sessions.get(&candidate.session_id)
            && record.sha256 == sha
        {
            report.skipped_unchanged += 1;
            continue;
        }

        if request.dry_run {
            report.uploaded += 1;
            report.entries.push(CloudSessionsSyncEntry {
                session_id: candidate.session_id,
                path: candidate.path.display().to_string(),
                status: "would-upload",
                error: None,
            });
            continue;
        }

        match run_jade_upload(
            &helper,
            &helper_env,
            &candidate.path,
            &user_id,
            request.profile.as_deref(),
            request.region.as_deref(),
            request.raw,
        ) {
            Ok(()) => {
                report.uploaded += 1;
                state.sessions.insert(
                    candidate.session_id.clone(),
                    CloudSessionsSyncRecord {
                        sha256: sha,
                        size: candidate.size,
                        modified_unix: candidate.modified_unix,
                        uploaded_at: chrono::Utc::now().to_rfc3339(),
                    },
                );
                state_dirty = true;
                report.entries.push(CloudSessionsSyncEntry {
                    session_id: candidate.session_id,
                    path: candidate.path.display().to_string(),
                    status: "uploaded",
                    error: None,
                });
            }
            Err(err) => {
                report.failed += 1;
                report.entries.push(CloudSessionsSyncEntry {
                    session_id: candidate.session_id,
                    path: candidate.path.display().to_string(),
                    status: "failed",
                    error: Some(err.to_string()),
                });
            }
        }
    }

    // Record completion time for non-dry runs (even if nothing changed) so
    // --min-interval-mins throttling works for schedulers, and persist any
    // newly uploaded session records.
    if !request.dry_run {
        state.last_sync_at = Some(chrono::Utc::now().to_rfc3339());
        save_cloud_sessions_sync_state(&state)?;
    } else if state_dirty {
        save_cloud_sessions_sync_state(&state)?;
    }

    if request.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        let verb = if request.dry_run {
            "Would upload"
        } else {
            "Uploaded"
        };
        println!("Jade cloud sessions sync ({})", report.sessions_dir);
        println!(
            "scanned: {}  {}: {}  unchanged: {}  failed: {}",
            report.scanned, verb, report.uploaded, report.skipped_unchanged, report.failed
        );
        if report.reached_max {
            println!("note: reached --max {}; rerun to continue", request.max);
        }
        for entry in &report.entries {
            match entry.error.as_deref() {
                Some(error) => println!("  [{}] {} ({})", entry.status, entry.session_id, error),
                None => println!("  [{}] {}", entry.status, entry.session_id),
            }
        }
    }

    if report.failed > 0 {
        anyhow::bail!("{} session(s) failed to upload", report.failed);
    }
    Ok(())
}

struct CloudSessionsDashboardRequest {
    limit: usize,
    output: Option<String>,
    open: bool,
    with_view: bool,
    user_id: String,
    profile: Option<String>,
    region: Option<String>,
    helper: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct CloudSessionListItem {
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    short_name: Option<String>,
    #[serde(default)]
    message_count: Option<serde_json::Value>,
    #[serde(default)]
    uploaded_at: Option<String>,
}

fn fetch_cloud_session_list_json(
    helper: &Path,
    helper_env: &[(&'static str, String)],
    user_id: &str,
    profile: Option<&str>,
    region: Option<&str>,
    limit: usize,
) -> Result<Vec<CloudSessionListItem>> {
    let mut args = vec!["list".to_string()];
    append_common_jade_args(
        &mut args,
        user_id.to_string(),
        profile.map(ToOwned::to_owned),
        region.map(ToOwned::to_owned),
    );
    args.extend(["--limit".to_string(), limit.to_string()]);
    args.push("--json".to_string());

    let output = ProcessCommand::new(helper)
        .args(&args)
        .envs(helper_env.iter().cloned())
        .stdin(Stdio::null())
        .output()
        .map_err(|err| anyhow::anyhow!("failed to run {}: {err}", helper.display()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.trim();
        anyhow::bail!(
            "{} list failed: {}",
            helper.display(),
            if detail.is_empty() {
                format!("exited with status {}", output.status)
            } else {
                detail.to_string()
            }
        );
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_cloud_session_list_json(stdout.trim())
}

/// Parse the helper's `list --json` output.
///
/// The Jade helper prints a top-level JSON array, but we also accept an object
/// wrapper keyed by `items` or `sessions` so the dashboard keeps working if the
/// helper's output shape changes.
fn parse_cloud_session_list_json(raw: &str) -> Result<Vec<CloudSessionListItem>> {
    let value: serde_json::Value = serde_json::from_str(raw)
        .map_err(|err| anyhow::anyhow!("failed to parse Jade list JSON: {err}"))?;
    let array = match value {
        serde_json::Value::Array(items) => items,
        serde_json::Value::Object(mut map) => map
            .remove("items")
            .or_else(|| map.remove("sessions"))
            .and_then(|value| match value {
                serde_json::Value::Array(items) => Some(items),
                _ => None,
            })
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "failed to parse Jade list JSON: expected an array or an object with an `items`/`sessions` array"
                )
            })?,
        other => anyhow::bail!(
            "failed to parse Jade list JSON: expected an array, found {}",
            json_value_kind(&other)
        ),
    };
    array
        .into_iter()
        .map(|item| {
            serde_json::from_value(item)
                .map_err(|err| anyhow::anyhow!("failed to parse Jade list item: {err}"))
        })
        .collect()
}

fn json_value_kind(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "a boolean",
        serde_json::Value::Number(_) => "a number",
        serde_json::Value::String(_) => "a string",
        serde_json::Value::Array(_) => "an array",
        serde_json::Value::Object(_) => "an object",
    }
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn message_count_label(value: Option<&serde_json::Value>) -> String {
    match value {
        Some(serde_json::Value::Number(num)) => num.to_string(),
        Some(serde_json::Value::String(text)) => text.clone(),
        _ => "-".to_string(),
    }
}

fn render_cloud_sessions_dashboard_html(
    user_id: &str,
    items: &[CloudSessionListItem],
    view_links: &std::collections::BTreeMap<String, String>,
) -> String {
    let generated = chrono::Utc::now().to_rfc3339();
    let mut rows = String::new();
    for item in items {
        let session_id = item.session_id.as_deref().unwrap_or("(unknown)");
        let title = item
            .title
            .as_deref()
            .filter(|value| !value.is_empty())
            .or(item.short_name.as_deref())
            .unwrap_or("(untitled)");
        let uploaded = item.uploaded_at.as_deref().unwrap_or("-");
        // When a local per-session viewer was generated, link the session id to it.
        let id_cell = match item.session_id.as_deref().and_then(|id| view_links.get(id)) {
            Some(link) => format!(
                "<a href='{}'>{}</a>",
                html_escape(link),
                html_escape(session_id)
            ),
            None => html_escape(session_id),
        };
        rows.push_str(&format!(
            "<tr><td class='id'>{}</td><td>{}</td><td class='num'>{}</td><td class='ts'>{}</td></tr>\n",
            id_cell,
            html_escape(title),
            html_escape(&message_count_label(item.message_count.as_ref())),
            html_escape(uploaded),
        ));
    }
    if rows.is_empty() {
        rows.push_str("<tr><td colspan='4' class='empty'>No uploaded sessions found.</td></tr>\n");
    }
    format!(
        "<!doctype html><meta charset='utf-8'>\n\
<title>Jade Cloud Sessions Dashboard</title>\n\
<style>body{{font-family:system-ui,sans-serif;max-width:1100px;margin:2rem auto;padding:0 1rem;color:#1b1b1f}}\
h1{{margin-bottom:0.2rem}}.meta{{color:#666;margin-bottom:1.5rem}}\
table{{border-collapse:collapse;width:100%}}th,td{{text-align:left;padding:0.5rem 0.6rem;border-bottom:1px solid #e3e3e8}}\
th{{background:#f6f8fa;position:sticky;top:0}}td.id{{font-family:ui-monospace,monospace;font-size:0.85rem}}\
td.id a{{color:#0a58ca;text-decoration:none}}td.id a:hover{{text-decoration:underline}}\
td.num{{text-align:right}}td.ts{{white-space:nowrap;color:#555}}td.empty{{text-align:center;color:#888;padding:2rem}}\
tr:hover td{{background:#fafbff}}</style>\n\
<h1>Jade Cloud Sessions</h1>\n\
<div class='meta'>user: {user} &middot; {count} session(s) &middot; generated {generated}</div>\n\
<table><thead><tr><th>Session ID</th><th>Title</th><th>Messages</th><th>Uploaded</th></tr></thead>\n\
<tbody>\n{rows}</tbody></table>\n",
        user = html_escape(user_id),
        count = items.len(),
        generated = html_escape(&generated),
        rows = rows,
    )
}

fn run_cloud_sessions_dashboard(request: CloudSessionsDashboardRequest) -> Result<()> {
    let config = load_cloud_sessions_config()?.unwrap_or_default();
    let helper_override = request.helper.clone().or_else(|| config.helper.clone());
    let helper = resolve_jade_sessions_helper(helper_override.as_deref())?;
    let helper_env = cloud_sessions_helper_env(&config);
    let user_id = config_or_default_user_id(request.user_id.clone(), &config);

    let items = fetch_cloud_session_list_json(
        &helper,
        &helper_env,
        &user_id,
        request.profile.as_deref(),
        request.region.as_deref(),
        request.limit,
    )?;

    let output_path = match request
        .output
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
    {
        Some(path) => expand_home_path(path),
        None => std::env::temp_dir().join(format!(
            "jade-cloud-dashboard-{}.html",
            chrono::Utc::now().format("%Y%m%d-%H%M%S")
        )),
    };
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Optionally download each session and render a local per-session viewer,
    // then link the dashboard rows to those files (relative to the dashboard).
    let mut view_links: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();
    if request.with_view {
        let views_dir = dashboard_views_dir(&output_path);
        std::fs::create_dir_all(&views_dir)?;
        let total = items.len();
        let mut generated = 0usize;
        for (idx, item) in items.iter().enumerate() {
            let Some(session_id) = item.session_id.as_deref().filter(|id| !id.is_empty()) else {
                continue;
            };
            let view_file = views_dir.join(format!("{}.html", sanitize_filename(session_id)));
            eprintln!("[{}/{}] downloading {}", idx + 1, total, session_id);
            match generate_cloud_session_view_html(
                &helper,
                &helper_env,
                &user_id,
                request.profile.as_deref(),
                request.region.as_deref(),
                session_id,
                &view_file,
            ) {
                Ok(()) => {
                    if let Some(rel) = relative_link(&output_path, &view_file) {
                        view_links.insert(session_id.to_string(), rel);
                        generated += 1;
                    }
                }
                Err(err) => {
                    eprintln!("  warning: could not render viewer for {session_id}: {err}");
                }
            }
        }
        eprintln!(
            "Generated {generated}/{total} per-session viewer(s) in {}",
            views_dir.display()
        );
    }

    let html = render_cloud_sessions_dashboard_html(&user_id, &items, &view_links);
    std::fs::write(&output_path, html.as_bytes())
        .map_err(|err| anyhow::anyhow!("failed to write {}: {err}", output_path.display()))?;

    println!(
        "Wrote Jade cloud sessions dashboard ({} session(s)) to {}",
        items.len(),
        output_path.display()
    );
    if request.open {
        let _ = open::that(&output_path);
    }
    Ok(())
}

/// Directory that holds per-session viewer HTML files for a dashboard.
fn dashboard_views_dir(dashboard_path: &Path) -> PathBuf {
    let stem = dashboard_path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "dashboard".to_string());
    let parent = dashboard_path.parent().unwrap_or_else(|| Path::new("."));
    parent.join(format!("{stem}-views"))
}

/// Make a filesystem-safe filename component from a session id.
fn sanitize_filename(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Build a link from the dashboard file to a viewer file, preferring a relative
/// path when both share a parent directory so the dashboard is portable.
fn relative_link(dashboard_path: &Path, view_file: &Path) -> Option<String> {
    let base = dashboard_path.parent()?;
    let rel = view_file.strip_prefix(base).ok()?;
    Some(rel.to_string_lossy().replace('\\', "/"))
}

/// Invoke the helper's `view --format html --output <file>` for one session.
fn generate_cloud_session_view_html(
    helper: &Path,
    helper_env: &[(&'static str, String)],
    user_id: &str,
    profile: Option<&str>,
    region: Option<&str>,
    session_id: &str,
    output_file: &Path,
) -> Result<()> {
    let mut args = vec!["view".to_string()];
    append_common_jade_args(
        &mut args,
        user_id.to_string(),
        profile.map(ToOwned::to_owned),
        region.map(ToOwned::to_owned),
    );
    args.extend(["--format".to_string(), "html".to_string()]);
    args.extend([
        "--output".to_string(),
        output_file.to_string_lossy().to_string(),
    ]);
    args.push(session_id.to_string());

    let output = ProcessCommand::new(helper)
        .args(&args)
        .envs(helper_env.iter().cloned())
        .stdin(Stdio::null())
        .output()
        .map_err(|err| anyhow::anyhow!("failed to run {}: {err}", helper.display()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.trim();
        anyhow::bail!(
            "{}",
            if detail.is_empty() {
                format!("view exited with status {}", output.status)
            } else {
                detail.to_string()
            }
        );
    }
    Ok(())
}

fn configured_label(value: Option<&str>) -> &str {
    value
        .filter(|value| !value.is_empty())
        .unwrap_or("not configured")
}

fn config_or_default_user_id(user_id: String, config: &CloudSessionsConfig) -> String {
    if user_id == "dev" {
        config.user_id.clone().unwrap_or(user_id)
    } else {
        user_id
    }
}

fn cloud_sessions_helper_env(config: &CloudSessionsConfig) -> Vec<(&'static str, String)> {
    let mut env = Vec::new();
    if let Some(api_base) = non_empty(config.api_base.clone()) {
        env.push(("JADE_API_BASE", api_base));
    }
    if let Some(api_token) = non_empty(config.api_token.clone()) {
        env.push(("JADE_API_TOKEN", api_token));
    }
    if let Some(api_token_id) = non_empty(config.api_token_id.clone()) {
        env.push(("JADE_API_TOKEN_ID", api_token_id));
    }
    env
}

fn cloud_sessions_helper_override(action: &CloudSessionsSubcommand) -> Option<String> {
    match action {
        CloudSessionsSubcommand::Configure { .. }
        | CloudSessionsSubcommand::Status { .. }
        | CloudSessionsSubcommand::Sync { .. } => None,
        CloudSessionsSubcommand::Upload { helper, .. }
        | CloudSessionsSubcommand::UploadLatest { helper, .. }
        | CloudSessionsSubcommand::List { helper, .. }
        | CloudSessionsSubcommand::Verify { helper, .. }
        | CloudSessionsSubcommand::Dashboard { helper, .. }
        | CloudSessionsSubcommand::View { helper, .. } => helper.clone(),
    }
}

fn append_common_jade_args(
    args: &mut Vec<String>,
    user_id: String,
    profile: Option<String>,
    region: Option<String>,
) {
    args.extend(["--user-id".to_string(), user_id]);
    if let Some(profile) = profile {
        args.extend(["--profile".to_string(), profile]);
    }
    if let Some(region) = region {
        args.extend(["--region".to_string(), region]);
    }
}

#[cfg(test)]
fn build_jade_sessions_args(action: CloudSessionsSubcommand) -> Vec<String> {
    build_jade_sessions_args_with_config(action, &CloudSessionsConfig::default())
}

fn build_jade_sessions_args_with_config(
    action: CloudSessionsSubcommand,
    config: &CloudSessionsConfig,
) -> Vec<String> {
    match action {
        CloudSessionsSubcommand::Configure { .. }
        | CloudSessionsSubcommand::Status { .. }
        | CloudSessionsSubcommand::Sync { .. }
        | CloudSessionsSubcommand::Dashboard { .. } => {
            unreachable!(
                "configure/status/sync/dashboard do not invoke the Jade helper via this builder"
            )
        }
        CloudSessionsSubcommand::Upload {
            session_file,
            raw,
            user_id,
            profile,
            region,
            ..
        } => {
            let mut args = vec!["upload".to_string()];
            append_common_jade_args(
                &mut args,
                config_or_default_user_id(user_id, config),
                profile,
                region,
            );
            if raw {
                args.push("--raw".to_string());
            }
            args.push(session_file);
            args
        }
        CloudSessionsSubcommand::UploadLatest {
            sessions_dir,
            raw,
            user_id,
            profile,
            region,
            ..
        } => {
            let mut args = vec!["upload-latest".to_string()];
            append_common_jade_args(
                &mut args,
                config_or_default_user_id(user_id, config),
                profile,
                region,
            );
            args.extend(["--sessions-dir".to_string(), sessions_dir]);
            if raw {
                args.push("--raw".to_string());
            }
            args
        }
        CloudSessionsSubcommand::List {
            limit,
            json,
            user_id,
            profile,
            region,
            ..
        } => {
            let mut args = vec!["list".to_string()];
            append_common_jade_args(
                &mut args,
                config_or_default_user_id(user_id, config),
                profile,
                region,
            );
            args.extend(["--limit".to_string(), limit.to_string()]);
            if json {
                args.push("--json".to_string());
            }
            args
        }
        CloudSessionsSubcommand::Verify {
            session_id,
            user_id,
            profile,
            region,
            ..
        } => {
            let mut args = vec!["verify".to_string()];
            append_common_jade_args(
                &mut args,
                config_or_default_user_id(user_id, config),
                profile,
                region,
            );
            args.push(session_id);
            args
        }
        CloudSessionsSubcommand::View {
            session_id,
            format,
            output,
            open,
            user_id,
            profile,
            region,
            ..
        } => {
            let mut args = vec!["view".to_string()];
            append_common_jade_args(
                &mut args,
                config_or_default_user_id(user_id, config),
                profile,
                region,
            );
            args.extend(["--format".to_string(), format]);
            if let Some(output) = output {
                args.extend(["--output".to_string(), output]);
            }
            if open {
                args.push("--open".to_string());
            }
            args.push(session_id);
            args
        }
    }
}

fn resolve_jade_sessions_helper(override_path: Option<&str>) -> Result<PathBuf> {
    if let Some(path) = override_path.map(str::trim).filter(|path| !path.is_empty()) {
        return Ok(PathBuf::from(path));
    }

    if let Some(path) = std::env::var_os("JCODE_JADE_SESSIONS_HELPER")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
    {
        return Ok(path);
    }

    let mut candidates = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("../jade/scripts/jade_sessions.py"));
        candidates.push(cwd.join("jade/scripts/jade_sessions.py"));
    }
    if let Some(home) = dirs::home_dir() {
        candidates.push(home.join("jade/scripts/jade_sessions.py"));
    }

    for candidate in candidates {
        if is_executable_file(&candidate) {
            return Ok(candidate);
        }
    }

    anyhow::bail!(
        "Could not find Jade session helper. Set --helper PATH or JCODE_JADE_SESSIONS_HELPER. Expected a private helper like ~/jade/scripts/jade_sessions.py"
    );
}

#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.is_file()
        && path
            .metadata()
            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}

pub async fn run_ambient_command(cmd: AmbientSubcommand) -> Result<()> {
    if let AmbientSubcommand::RunVisible = cmd {
        return run_ambient_visible().await;
    }

    let debug_cmd = match cmd {
        AmbientSubcommand::Status => "ambient:status",
        AmbientSubcommand::Log => "ambient:log",
        AmbientSubcommand::Trigger => "ambient:trigger",
        AmbientSubcommand::Stop => "ambient:stop",
        AmbientSubcommand::RunVisible => unreachable!(),
    };

    super::debug::run_debug_command(debug_cmd, "", None, None, false).await
}

pub async fn run_transcript_command(
    text: Option<String>,
    mode: crate::protocol::TranscriptMode,
    session: Option<String>,
) -> Result<()> {
    let text = if let Some(text) = text {
        text
    } else {
        let mut stdin = String::new();
        std::io::stdin().read_to_string(&mut stdin)?;
        let trimmed = stdin.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            anyhow::bail!("Provide transcript text as an argument or pipe it via stdin")
        }
        trimmed.to_string()
    };

    let mut client = crate::server::Client::connect_debug().await?;
    let request_id = client.send_transcript(&text, mode, session).await?;

    loop {
        match client.read_event().await? {
            crate::protocol::ServerEvent::Ack { id } if id == request_id => {}
            crate::protocol::ServerEvent::Done { id } if id == request_id => return Ok(()),
            crate::protocol::ServerEvent::Error { id, message, .. } if id == request_id => {
                anyhow::bail!(message)
            }
            _ => {}
        }
    }
}

pub async fn run_dictate_command(type_output: bool) -> Result<()> {
    let run = crate::dictation::run_configured().await?;

    if type_output {
        crate::dictation::type_text(&run.text)
    } else {
        run_transcript_command(Some(run.text), run.mode, None).await
    }
}

#[derive(Serialize)]
struct SessionRenameOutput {
    session_id: String,
    display_name: String,
    title: Option<String>,
    cleared: bool,
}

pub fn run_session_rename_command(
    session_ref: &str,
    name: Option<&str>,
    clear: bool,
    json: bool,
) -> Result<()> {
    let resolved_id = session::find_session_by_name_or_id(session_ref)?;
    let mut session = session::Session::load(&resolved_id)?;

    if clear {
        session.rename_title(None);
    } else {
        let Some(name) = name.map(str::trim).filter(|name| !name.is_empty()) else {
            anyhow::bail!("Provide a session name or use --clear");
        };
        session.rename_title(Some(name.to_string()));
    }

    session.save()?;
    crate::tui::session_picker::invalidate_session_list_cache();

    let output = SessionRenameOutput {
        session_id: session.id.clone(),
        display_name: session.display_name().to_string(),
        title: session.display_title().map(ToOwned::to_owned),
        cleared: clear,
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else if clear {
        println!(
            "Cleared custom name for session {} ({}).",
            output.display_name, output.session_id
        );
    } else if let Some(title) = output.title.as_deref() {
        println!(
            "Renamed session {} ({}) to \"{}\".",
            output.display_name, output.session_id, title
        );
    }

    Ok(())
}

async fn run_ambient_visible() -> Result<()> {
    use crate::ambient::VisibleCycleContext;

    let context = VisibleCycleContext::load().map_err(|e| {
        anyhow::anyhow!(
            "Failed to load visible cycle context: {}\nIs the ambient runner running?",
            e
        )
    })?;

    let (provider, registry) = super::provider_init::init_provider_and_registry(
        &super::provider_init::ProviderChoice::Auto,
        None,
    )
    .await?;

    registry.register_ambient_tools().await;

    let safety = std::sync::Arc::new(crate::safety::SafetySystem::new());
    crate::tool::ambient::init_safety_system(safety);

    let (terminal, tui_runtime) = init_tui_runtime()?;

    let mut app = tui::App::new(provider, registry);
    app.set_ambient_mode(context.system_prompt, context.initial_message);

    let _ = crossterm::execute!(
        std::io::stdout(),
        crossterm::terminal::SetTitle("🤖 jcode ambient cycle")
    );

    let result = app.run(terminal).await;

    tui_runtime.finish(true);

    if let Some(cycle_result) = crate::tool::ambient::take_cycle_result() {
        let result_path = VisibleCycleContext::result_path()?;
        crate::storage::write_json(&result_path, &cycle_result)?;
        eprintln!("Ambient cycle result saved.");
    }

    result?;
    Ok(())
}

pub enum MemorySubcommand {
    List {
        scope: String,
        tag: Option<String>,
    },
    Search {
        query: String,
        semantic: bool,
    },
    Export {
        output: String,
        scope: String,
    },
    Import {
        input: String,
        scope: String,
        overwrite: bool,
    },
    Stats,
    ClearTest,
}

pub fn run_memory_command(cmd: MemorySubcommand) -> Result<()> {
    use memory::{MemoryEntry, MemoryManager};

    let manager = MemoryManager::new();

    match cmd {
        MemorySubcommand::List { scope, tag } => {
            let mut all_memories: Vec<MemoryEntry> = Vec::new();

            if (scope == "all" || scope == "project")
                && let Ok(graph) = manager.load_project_graph()
            {
                all_memories.extend(graph.all_memories().cloned());
            }
            if (scope == "all" || scope == "global")
                && let Ok(graph) = manager.load_global_graph()
            {
                all_memories.extend(graph.all_memories().cloned());
            }

            if let Some(tag_filter) = tag {
                all_memories.retain(|m| m.tags.contains(&tag_filter));
            }

            all_memories.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));

            if all_memories.is_empty() {
                println!("No memories found.");
            } else {
                println!("Found {} memories:\n", all_memories.len());
                for entry in &all_memories {
                    let tags_str = if entry.tags.is_empty() {
                        String::new()
                    } else {
                        format!(" [{}]", entry.tags.join(", "))
                    };
                    let conf = entry.effective_confidence();
                    println!(
                        "- [{}] {}{}\n  id: {} (conf: {:.0}%, accessed: {}x)",
                        entry.category,
                        entry.content,
                        tags_str,
                        entry.id,
                        conf * 100.0,
                        entry.access_count
                    );
                    println!();
                }
            }
        }

        MemorySubcommand::Search { query, semantic } => {
            if semantic {
                match manager.find_similar(&query, 0.3, 20) {
                    Ok(results) => {
                        if results.is_empty() {
                            println!("No memories found matching '{}'", query);
                        } else {
                            println!(
                                "Found {} memories matching '{}' (semantic):\n",
                                results.len(),
                                query
                            );
                            for (entry, score) in results {
                                let tags_str = if entry.tags.is_empty() {
                                    String::new()
                                } else {
                                    format!(" [{}]", entry.tags.join(", "))
                                };
                                println!(
                                    "- [{}] {}{}\n  id: {} (score: {:.0}%)",
                                    entry.category,
                                    entry.content,
                                    tags_str,
                                    entry.id,
                                    score * 100.0
                                );
                                println!();
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("Search failed: {}", e);
                    }
                }
            } else {
                match manager.search(&query) {
                    Ok(results) => {
                        if results.is_empty() {
                            println!("No memories found matching '{}'", query);
                        } else {
                            println!(
                                "Found {} memories matching '{}' (keyword):\n",
                                results.len(),
                                query
                            );
                            for entry in results {
                                let tags_str = if entry.tags.is_empty() {
                                    String::new()
                                } else {
                                    format!(" [{}]", entry.tags.join(", "))
                                };
                                println!(
                                    "- [{}] {}{}\n  id: {}",
                                    entry.category, entry.content, tags_str, entry.id
                                );
                                println!();
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("Search failed: {}", e);
                    }
                }
            }
        }

        MemorySubcommand::Export { output, scope } => {
            let mut all_memories: Vec<memory::MemoryEntry> = Vec::new();

            if (scope == "all" || scope == "project")
                && let Ok(graph) = manager.load_project_graph()
            {
                all_memories.extend(graph.all_memories().cloned());
            }
            if (scope == "all" || scope == "global")
                && let Ok(graph) = manager.load_global_graph()
            {
                all_memories.extend(graph.all_memories().cloned());
            }

            let json = serde_json::to_string_pretty(&all_memories)?;
            std::fs::write(&output, json)?;
            println!("Exported {} memories to {}", all_memories.len(), output);
        }

        MemorySubcommand::Import {
            input,
            scope,
            overwrite,
        } => {
            let content = std::fs::read_to_string(&input)?;
            let memories: Vec<memory::MemoryEntry> = serde_json::from_str(&content)?;

            let mut imported = 0;
            let mut skipped = 0;

            for entry in memories {
                let result = if scope == "global" {
                    if !overwrite
                        && let Ok(graph) = manager.load_global_graph()
                        && graph.get_memory(&entry.id).is_some()
                    {
                        skipped += 1;
                        continue;
                    }
                    manager.remember_global(entry)
                } else {
                    if !overwrite
                        && let Ok(graph) = manager.load_project_graph()
                        && graph.get_memory(&entry.id).is_some()
                    {
                        skipped += 1;
                        continue;
                    }
                    manager.remember_project(entry)
                };

                if result.is_ok() {
                    imported += 1;
                }
            }

            println!("Imported {} memories ({} skipped)", imported, skipped);
        }

        MemorySubcommand::Stats => {
            let mut project_count = 0;
            let mut global_count = 0;
            let mut total_tags = std::collections::HashSet::new();
            let mut categories: std::collections::HashMap<String, usize> =
                std::collections::HashMap::new();

            if let Ok(graph) = manager.load_project_graph() {
                project_count = graph.memory_count();
                for entry in graph.all_memories() {
                    for tag in &entry.tags {
                        total_tags.insert(tag.clone());
                    }
                    *categories.entry(entry.category.to_string()).or_default() += 1;
                }
            }

            if let Ok(graph) = manager.load_global_graph() {
                global_count = graph.memory_count();
                for entry in graph.all_memories() {
                    for tag in &entry.tags {
                        total_tags.insert(tag.clone());
                    }
                    *categories.entry(entry.category.to_string()).or_default() += 1;
                }
            }

            println!("Memory Statistics:");
            println!("  Project memories: {}", project_count);
            println!("  Global memories:  {}", global_count);
            println!("  Total:            {}", project_count + global_count);
            println!("  Unique tags:      {}", total_tags.len());
            println!("\nBy category:");
            for (cat, count) in &categories {
                println!("  {}: {}", cat, count);
            }
        }

        MemorySubcommand::ClearTest => {
            let test_dir = storage::jcode_dir()?.join("memory").join("test");
            if test_dir.exists() {
                let count = std::fs::read_dir(&test_dir)?.count();
                std::fs::remove_dir_all(&test_dir)?;
                println!("Cleared test memory storage ({} files)", count);
            } else {
                println!("Test memory storage is already empty");
            }
        }
    }

    Ok(())
}

pub fn run_pair_command(list: bool, revoke: Option<String>) -> Result<()> {
    let mut registry = gateway::DeviceRegistry::load();

    if list {
        if registry.devices.is_empty() {
            eprintln!("No paired devices.");
        } else {
            eprintln!("\x1b[1mPaired devices:\x1b[0m\n");
            for device in &registry.devices {
                let last_seen = &device.last_seen;
                eprintln!("  \x1b[36m{}\x1b[0m  ({})", device.name, device.id);
                eprintln!("    Paired: {}  Last seen: {}", device.paired_at, last_seen);
                if let Some(ref apns) = device.apns_token {
                    eprintln!("    APNs: {}...", &apns[..apns.len().min(16)]);
                }
                eprintln!();
            }
        }
        return Ok(());
    }

    if let Some(ref target) = revoke {
        let before = registry.devices.len();
        registry
            .devices
            .retain(|d| d.id != *target && d.name != *target);
        if registry.devices.len() < before {
            registry.save()?;
            eprintln!("\x1b[32m✓\x1b[0m Revoked device: {}", target);
        } else {
            eprintln!("\x1b[31m✗\x1b[0m No device found matching: {}", target);
        }
        return Ok(());
    }

    let gw_config = &crate::config::config().gateway;

    if !gw_config.enabled {
        eprintln!("\x1b[33m⚠\x1b[0m  Gateway is disabled. Enable it in ~/.jcode/config.toml:\n");
        eprintln!("    \x1b[2m[gateway]\x1b[0m");
        eprintln!("    \x1b[2menabled = true\x1b[0m");
        eprintln!("    \x1b[2mport = {}\x1b[0m\n", gw_config.port);
        eprintln!("  Then restart the jcode server.\n");
    }

    let code = registry.generate_pairing_code();
    let connect_host = resolve_connect_host(&gw_config.bind_addr);
    let pair_uri = format!(
        "jcode://pair?host={}&port={}&code={}",
        connect_host, gw_config.port, code
    );

    eprintln!();
    eprintln!("  \x1b[1mScan with the jcode iOS app:\x1b[0m\n");
    match crate::login_qr::render_unicode_qr(&pair_uri) {
        Ok(qr) => {
            for line in qr.lines() {
                eprintln!("  {line}");
            }
        }
        Err(_) => eprintln!("  \x1b[33m(QR code generation failed)\x1b[0m"),
    }
    eprintln!();
    eprintln!(
        "  Pairing code:  \x1b[1;37m{} {}\x1b[0m   \x1b[2m(expires in 5 minutes)\x1b[0m",
        &code[..3],
        &code[3..]
    );
    let resolved_hint = format!("{}:{}", connect_host, gw_config.port);
    let bind_hint = format!("{}:{}", gw_config.bind_addr, gw_config.port);
    eprintln!("  Connect host:  \x1b[36m{}\x1b[0m", resolved_hint);
    if connect_host != gw_config.bind_addr {
        eprintln!("  Bind address:  \x1b[2m{}\x1b[0m", bind_hint);
    }

    if connect_host == "<your-mac-hostname>" {
        eprintln!(
            "\n  \x1b[33mTip:\x1b[0m set JCODE_GATEWAY_HOST to your reachable Tailscale hostname."
        );
    }

    if (gw_config.bind_addr.as_str(), gw_config.port)
        .to_socket_addrs()
        .ok()
        .and_then(|mut it| it.next())
        .is_none()
    {
        eprintln!(
            "  \x1b[33mWarning:\x1b[0m gateway bind address appears invalid: {}",
            bind_hint
        );
    }
    eprintln!();

    Ok(())
}

pub fn resolve_connect_host(bind_addr: &str) -> String {
    if bind_addr == "0.0.0.0" || bind_addr == "::" {
        if let Some(host) = std::env::var("JCODE_GATEWAY_HOST")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
        {
            return host;
        }

        if let Some(host) = detect_tailscale_dns_name() {
            return host;
        }

        return std::env::var("HOSTNAME")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "<your-mac-hostname>".to_string());
    }
    bind_addr.to_string()
}

pub fn parse_tailscale_dns_name(status_json: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(status_json).ok()?;
    let dns_name = value
        .get("Self")?
        .get("DNSName")?
        .as_str()?
        .trim()
        .trim_end_matches('.')
        .to_string();

    if dns_name.is_empty() {
        None
    } else {
        Some(dns_name)
    }
}

pub fn detect_tailscale_dns_name() -> Option<String> {
    let output = std::process::Command::new("tailscale")
        .args(["status", "--json"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    parse_tailscale_dns_name(&output.stdout)
}

pub async fn run_browser(action: &str) -> Result<()> {
    match action {
        "setup" => browser::run_setup_command().await?,
        "status" => {
            let status = browser::ensure_browser_ready_noninteractive().await?;
            println!("Browser automation");
            println!("  backend: {}", status.backend);
            println!("  browser: {}", status.browser);
            println!(
                "  binary: {}",
                if status.binary_installed {
                    "installed"
                } else {
                    "missing"
                }
            );
            println!(
                "  setup: {}",
                if status.setup_complete {
                    "complete"
                } else {
                    "not complete"
                }
            );
            println!(
                "  bridge: {}",
                if status.responding {
                    "responding"
                } else {
                    "not responding"
                }
            );
            println!(
                "  compatibility: {}",
                if status.compatible {
                    "ok"
                } else {
                    "extension/bridge mismatch"
                }
            );
            if !status.missing_actions.is_empty() {
                println!("  missing actions: {}", status.missing_actions.join(", "));
            }

            if status.ready {
                println!("\nBuilt-in browser tool is ready.");
            } else if status.responding && !status.compatible {
                println!(
                    "\nThe browser bridge is connected, but the installed Firefox extension is out of date for this jcode build. Run `jcode browser setup` to repair or update it."
                );
            } else {
                println!("\nRun `jcode browser setup` to install or repair it.");
            }
        }
        other => {
            eprintln!("Unknown browser action: {}", other);
            eprintln!("Available: setup, status");
            std::process::exit(1);
        }
    }
    Ok(())
}

#[derive(Debug, Serialize)]
struct ModelListReport {
    provider: String,
    selected_model: String,
    models: Vec<String>,
    routes: Vec<ModelListRouteReport>,
}

#[derive(Debug, Serialize)]
struct ModelListRouteReport {
    provider: String,
    model: String,
    method: String,
    available: bool,
}

#[derive(Debug, Serialize)]
struct RunCommandReport {
    session_id: String,
    provider: String,
    model: String,
    text: String,
    usage: crate::agent::TokenUsage,
}

#[derive(Debug, Default)]
struct NdjsonRunState {
    text: String,
    session_id: Option<String>,
    upstream_provider: Option<String>,
    connection_type: Option<String>,
    connection_phase: Option<String>,
    status_detail: Option<String>,
    usage: crate::agent::TokenUsage,
}

pub fn run_auth_status_command(emit_json: bool) -> Result<()> {
    report_info::run_auth_status_command(emit_json)
}

pub async fn run_auth_doctor_command(
    provider_arg: Option<&str>,
    validate: bool,
    emit_json: bool,
) -> Result<()> {
    report_info::run_auth_doctor_command(provider_arg, validate, emit_json).await
}

pub fn run_provider_list_command(emit_json: bool) -> Result<()> {
    report_info::run_provider_list_command(emit_json)
}

pub async fn run_provider_current_command(
    choice: &super::provider_init::ProviderChoice,
    model: Option<&str>,
    emit_json: bool,
) -> Result<()> {
    report_info::run_provider_current_command(choice, model, emit_json).await
}

pub fn run_version_command(emit_json: bool) -> Result<()> {
    report_info::run_version_command(emit_json)
}

pub async fn run_usage_command(emit_json: bool) -> Result<()> {
    report_info::run_usage_command(emit_json).await
}

/// Gracefully reload the running background server onto the newest binary.
///
/// This is the preferred upgrade path (issue #291): instead of killing the
/// daemon and dropping live headless/swarm sessions, we ask it to hand its
/// sessions off to a freshly exec'd server (the same path `/reload` uses).
///
/// Behavior:
/// - With `force == false` (the default), the server only reloads when it is
///   provably running older code than an available reload candidate. A server
///   already on the newest binary reports "already up to date" and does
///   nothing, which keeps an installer from downgrading a newer/dev daemon or
///   re-entering the reload-loop family (#277).
/// - With `force == true`, the server reloads unconditionally.
/// - If no server is running, this is a successful no-op so installers can call
///   it unconditionally.
pub async fn run_server_reload_command(force: bool, emit_json: bool) -> Result<()> {
    use crate::protocol::ServerEvent;
    use std::time::Duration;

    let socket = crate::server::socket_path();

    #[derive(Serialize)]
    struct ServerReloadReport {
        socket: String,
        had_listener: bool,
        forced: bool,
        reloaded: bool,
        already_current: bool,
        handoff_ready: bool,
        detail: String,
    }

    let emit = |report: ServerReloadReport| -> Result<()> {
        if emit_json {
            println!("{}", serde_json::to_string_pretty(&report)?);
        } else if !report.detail.is_empty() {
            println!("{}", report.detail);
        }
        Ok(())
    };

    // No server? Nothing to reload. This is a success so an installer can call
    // `jcode server reload` unconditionally after swapping the binary.
    if !crate::server::has_live_listener(&socket).await {
        // Reap a stale socket left by a crashed daemon so the next launch binds
        // cleanly instead of wedging in a connect-retry loop.
        let reaped = crate::server::reap_stale_socket_if_dead(&socket).await;
        let detail = if reaped {
            "No running jcode server found; cleared a stale socket.".to_string()
        } else {
            "No running jcode server found; nothing to reload.".to_string()
        };
        return emit(ServerReloadReport {
            socket: socket.display().to_string(),
            had_listener: false,
            forced: force,
            reloaded: false,
            already_current: false,
            handoff_ready: false,
            detail,
        });
    }

    let mut client = crate::server::Client::connect().await?;

    // Before asking the (possibly older) daemon to reload, repair a stale
    // `shared-server` channel from the client side. The running server resolves
    // its reload target from that channel; if it still points at the server's
    // own old binary (the "current client, stale server" state, e.g. after a
    // no-op `/update`), a forced reload would just re-exec the same old binary.
    // Repointing shared-server -> stable when stable is strictly newer gives the
    // reload a newer binary to exec into. Never downgrades; preserves a fresher
    // self-dev pin. Best-effort: a failure here must not block the reload.
    match crate::build::repair_stale_shared_server_channel() {
        Ok(crate::build::SharedServerRepair::Repaired {
            repaired_to,
            previous,
        }) => {
            crate::logging::info(&format!(
                "server reload: repaired stale shared-server channel {:?} -> {} before reload",
                previous, repaired_to
            ));
        }
        Ok(crate::build::SharedServerRepair::AlreadyCurrent) => {}
        Err(err) => {
            crate::logging::warn(&format!(
                "server reload: shared-server channel repair failed (continuing): {}",
                err
            ));
        }
    }

    let request_id = client.reload_with_force(force).await?;

    let mut reloading = false;
    let mut skipped = false;

    // Drive the request to a terminal state. On a real reload the old server
    // exec's a new process, which drops this connection after it sends Done;
    // we treat a disconnect after observing Reloading as the expected handoff.
    loop {
        match client.read_event().await {
            Ok(ServerEvent::Ack { id }) if id == request_id => {}
            Ok(ServerEvent::Reloading { .. }) => {
                reloading = true;
            }
            Ok(ServerEvent::ReloadProgress { step, .. }) if step == "skip" => {
                skipped = true;
            }
            Ok(ServerEvent::ReloadProgress { .. }) => {}
            Ok(ServerEvent::Done { id }) if id == request_id => break,
            Ok(ServerEvent::Error { id, message, .. }) if id == request_id => {
                anyhow::bail!("server reload failed: {message}");
            }
            Ok(_) => {}
            Err(e) => {
                // A disconnect mid-reload is the expected handoff; otherwise it
                // is a genuine failure.
                if reloading {
                    break;
                }
                return Err(e);
            }
        }
    }

    if skipped && !reloading {
        return emit(ServerReloadReport {
            socket: socket.display().to_string(),
            had_listener: true,
            forced: force,
            reloaded: false,
            already_current: true,
            handoff_ready: true,
            detail: "jcode server is already running the newest binary; no reload needed."
                .to_string(),
        });
    }

    // Wait (bounded) for the freshly exec'd server to take over the socket so
    // callers know the upgrade actually landed.
    let handoff_ready = matches!(
        crate::server::await_reload_handoff(&socket, Duration::from_secs(30)).await,
        crate::server::ReloadWaitStatus::Ready
    );

    let detail = if handoff_ready {
        "jcode server reloaded onto the newest binary.".to_string()
    } else {
        "jcode server reload requested; the new server is still coming up.".to_string()
    };

    emit(ServerReloadReport {
        socket: socket.display().to_string(),
        had_listener: true,
        forced: force,
        reloaded: true,
        already_current: false,
        handoff_ready,
        detail,
    })
}

/// Stop the running background server gracefully and clear its socket.
///
/// Intended for use after an upgrade so the next launch starts the freshly
/// installed binary instead of a surviving daemon running old code (issue #291).
///
/// Steps:
/// 1. Look up the daemon owning the active socket in the server registry and
///    send it SIGTERM (the daemon has a graceful SIGTERM handler).
/// 2. Wait for the listener to go away (bounded), escalating to SIGKILL only if
///    the process refuses to exit.
/// 3. Reap any leftover stale socket so a later launch binds cleanly.
pub async fn run_server_stop_command(force: bool, emit_json: bool) -> Result<()> {
    use std::time::{Duration, Instant};

    if !force {
        let msg = "`jcode server stop` terminates the daemon and drops any live headless/swarm sessions. \
Prefer `jcode server reload` to pick up an upgrade gracefully. \
Re-run with `--force` if you really want to stop the server.";
        if emit_json {
            println!(
                "{}",
                serde_json::json!({
                    "stopped": false,
                    "force_required": true,
                    "detail": msg,
                })
            );
        } else {
            eprintln!("{msg}");
        }
        return Ok(());
    }

    let socket = crate::server::socket_path();
    let had_listener = crate::server::has_live_listener(&socket).await;
    let server_info = crate::registry::find_server_by_socket_sync(&socket);

    #[derive(Serialize)]
    struct ServerStopReport {
        socket: String,
        had_listener: bool,
        signaled_pid: Option<u32>,
        stopped: bool,
        reaped_socket: bool,
        detail: String,
    }

    let mut signaled_pid: Option<u32> = None;
    let mut stopped = false;
    let detail: String;

    if let Some(info) = server_info.as_ref() {
        let pid = info.pid;
        if crate::platform::is_process_running(pid) {
            #[cfg(unix)]
            {
                // The daemon spawns detached with setsid(), so it leads its own
                // process group. Signal the group so any helper children exit too.
                match crate::platform::signal_detached_process_group(pid, libc::SIGTERM) {
                    Ok(()) => {
                        signaled_pid = Some(pid);
                        detail = format!("Sent SIGTERM to jcode server (pid {pid}).");
                    }
                    Err(e) => {
                        detail = format!("Failed to signal jcode server (pid {pid}): {e}");
                    }
                }
            }
            #[cfg(not(unix))]
            {
                match crate::platform::signal_detached_process_group(pid, 0) {
                    Ok(()) => {
                        signaled_pid = Some(pid);
                        detail = format!("Terminated jcode server (pid {pid}).");
                    }
                    Err(e) => {
                        detail = format!("Failed to terminate jcode server (pid {pid}): {e}");
                    }
                }
            }
        } else {
            detail = format!("Registered jcode server (pid {pid}) is not running.");
        }
    } else if had_listener {
        // A listener answers but no registry entry maps to it. We deliberately
        // do not guess a pid; just reap the socket below once the listener is
        // gone. (This is rare: a daemon that bound the socket but never wrote a
        // registry entry.)
        detail = "Found a live server socket with no registry entry.".to_string();
    } else {
        detail = "No running jcode server found.".to_string();
    }

    // Wait for the listener to disappear after signalling. Escalate to SIGKILL
    // once if the daemon does not exit within the graceful window.
    if signaled_pid.is_some() || had_listener {
        let deadline = Instant::now() + Duration::from_secs(5);
        #[cfg(unix)]
        let mut escalated = false;
        loop {
            let listener_gone = !crate::server::has_live_listener(&socket).await;
            let process_gone = signaled_pid
                .map(|pid| !crate::platform::is_process_running(pid))
                .unwrap_or(true);
            if listener_gone && process_gone {
                stopped = true;
                break;
            }
            if Instant::now() >= deadline {
                break;
            }
            #[cfg(unix)]
            if !escalated
                && Instant::now() + Duration::from_secs(2) >= deadline
                && let Some(pid) = signaled_pid
                && crate::platform::is_process_running(pid)
            {
                let _ = crate::platform::signal_detached_process_group(pid, libc::SIGKILL);
                escalated = true;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    } else {
        stopped = true;
    }

    // Reap any stale socket the (now-dead) daemon left behind so the next launch
    // binds cleanly instead of wedging in a connect-retry loop.
    let reaped = crate::server::reap_stale_socket_if_dead(&socket).await;

    if emit_json {
        let report = ServerStopReport {
            socket: socket.display().to_string(),
            had_listener,
            signaled_pid,
            stopped,
            reaped_socket: reaped,
            detail: detail.clone(),
        };
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        if !detail.is_empty() {
            println!("{detail}");
        }
        if stopped && signaled_pid.is_some() {
            println!("jcode server stopped.");
        } else if stopped && !had_listener && signaled_pid.is_none() {
            // Nothing was running; this is still a success for an installer.
        } else if !stopped {
            println!(
                "jcode server did not exit cleanly; it may still be shutting down. Re-run if needed."
            );
        }
        if reaped {
            println!("Cleared a stale jcode socket.");
        }
    }

    Ok(())
}

pub async fn run_single_message_command(
    choice: &super::provider_init::ProviderChoice,
    model: Option<&str>,
    resume_session: Option<&str>,
    message: &str,
    emit_json: bool,
    emit_ndjson: bool,
) -> Result<()> {
    let provider = if emit_json || emit_ndjson {
        super::provider_init::init_provider_quiet(choice, model).await?
    } else {
        super::provider_init::init_provider_for_validation(choice, model).await?
    };
    let registry = crate::tool::Registry::new(provider.clone()).await;
    // Load MCP servers from ~/.jcode/mcp.json so headless `jcode run` has the
    // same `mcp__*` tools as interactive/server sessions. This is non-blocking:
    // `register_mcp_tools` advertises cached tool schemas synchronously (so the
    // first locked tool snapshot already contains MCP tools, for zero
    // prompt-cache miss) and connects in the background (connect-on-first-call).
    // For a short single-message run, startup latency is unchanged.
    // (#390, #206 Phase 2)
    if run_command_mcp_enabled() {
        registry.register_mcp_tools(None, None, None).await;
        // Cold-cache gap: when a configured MCP server has no cached schema yet
        // (first ever use, or reconfigured), advertise-early registers nothing
        // for it, and a single-turn `jcode run` locks its tool snapshot before
        // the background connection finishes, so the model would never see those
        // tools. Long-lived sessions recover on a later turn, but `jcode run`
        // has no later turn. So, only when the cache is cold for some configured
        // server, briefly wait for the first connection to register tools before
        // the agent runs. Warm runs skip this entirely and stay instant. (#390)
        wait_for_cold_cache_mcp_tools(&registry).await;
    }
    let mut agent = crate::agent::Agent::new(provider.clone(), registry);
    restore_agent_session_if_requested(&mut agent, resume_session)?;

    if emit_json {
        let text = run_single_message_command_capture_with_auto_poke(&mut agent, message).await?;
        let report = RunCommandReport {
            session_id: agent.session_id().to_string(),
            provider: provider.name().to_string(),
            model: provider.model(),
            text,
            usage: agent.last_usage().clone(),
        };
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else if emit_ndjson {
        run_single_message_command_ndjson(&mut agent, provider.clone(), message).await?;
    } else {
        run_single_message_command_plain_with_auto_poke(&mut agent, message).await?;
    }

    Ok(())
}

fn run_command_auto_poke_enabled() -> bool {
    std::env::var("JCODE_RUN_AUTO_POKE")
        .ok()
        .map(|value| {
            let value = value.trim().to_ascii_lowercase();
            !matches!(value.as_str(), "0" | "false" | "off" | "no")
        })
        .unwrap_or(true)
}

/// Whether headless `jcode run` should load MCP servers from `~/.jcode/mcp.json`.
/// Enabled by default; set `JCODE_RUN_MCP=0` (or `false`/`off`/`no`) to skip MCP
/// registration for latency-sensitive scripting. (#390)
fn run_command_mcp_enabled() -> bool {
    std::env::var("JCODE_RUN_MCP")
        .ok()
        .map(|value| {
            let value = value.trim().to_ascii_lowercase();
            !matches!(value.as_str(), "0" | "false" | "off" | "no")
        })
        .unwrap_or(true)
}

/// Max time `jcode run` waits for cold-cache MCP servers to register their
/// tools before running the single turn. Override with `JCODE_RUN_MCP_WAIT_MS`
/// (0 disables the wait).
fn run_command_mcp_cold_wait() -> std::time::Duration {
    let ms = std::env::var("JCODE_RUN_MCP_WAIT_MS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(5000);
    std::time::Duration::from_millis(ms)
}

/// Returns the set of MCP servers configured for this run that have no usable
/// cached schema yet (cold cache). Advertise-early can only pre-register tools
/// for servers whose schemas are cached, so these are the servers whose tools
/// would otherwise miss the single-turn snapshot.
fn cold_cache_mcp_servers() -> Vec<String> {
    let config = crate::mcp::McpConfig::load();
    if config.servers.is_empty() {
        return Vec::new();
    }
    let cache = crate::mcp::McpSchemaCache::load();
    config
        .servers
        .iter()
        .filter(|(name, cfg)| cache.tools_for(name, cfg).is_none())
        .map(|(name, _)| name.clone())
        .collect()
}

/// Bridge the cold-cache gap for `jcode run`: if any configured MCP server has
/// no cached schema, briefly poll the registry until its `mcp__*` tools appear
/// (or the budget elapses) so the single turn's locked tool snapshot includes
/// them. Warm caches return immediately because `cold_cache_mcp_servers` is
/// empty. (#390)
async fn wait_for_cold_cache_mcp_tools(registry: &crate::tool::Registry) {
    let cold_servers = cold_cache_mcp_servers();
    if cold_servers.is_empty() {
        return;
    }
    let budget = run_command_mcp_cold_wait();
    if budget.is_zero() {
        return;
    }
    crate::logging::info(&format!(
        "jcode run: waiting up to {}ms for cold-cache MCP server(s) to register tools: {}",
        budget.as_millis(),
        cold_servers.join(", ")
    ));
    let deadline = std::time::Instant::now() + budget;
    loop {
        let names = registry.tool_names().await;
        let covered = cold_servers.iter().all(|server| {
            let prefix = format!("mcp__{}__", server);
            names.iter().any(|name| name.starts_with(&prefix))
        });
        if covered {
            crate::logging::info(
                "jcode run: cold-cache MCP server(s) registered tools; proceeding",
            );
            return;
        }
        if std::time::Instant::now() >= deadline {
            crate::logging::warn(
                "jcode run: timed out waiting for cold-cache MCP server(s); \
                 their tools may be missing from this run",
            );
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

fn run_command_auto_poke_max_turns() -> Option<usize> {
    std::env::var("JCODE_RUN_AUTO_POKE_MAX_TURNS")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
}

fn run_command_auto_poke_limit_reached(turns_completed: usize, max_turns: Option<usize>) -> bool {
    max_turns
        .map(|max_turns| turns_completed >= max_turns)
        .unwrap_or(false)
}

const RUN_TODO_CONFIDENCE_THRESHOLD: u8 = 90;

enum RunAutoPokeFollowUp {
    Incomplete {
        count: usize,
        message: String,
    },
    ConfidenceSummary {
        total_todos: usize,
        message: String,
        confidence_spike_challenge: bool,
    },
}

fn run_todos(session_id: &str) -> Vec<crate::todo::TodoItem> {
    crate::todo::load_todos(session_id).unwrap_or_default()
}

fn build_run_auto_poke_follow_up_from_todos(
    todos: &[crate::todo::TodoItem],
    confidence_spike_challenged: bool,
) -> Option<RunAutoPokeFollowUp> {
    let incomplete: Vec<_> = todos
        .iter()
        .filter(|todo| todo.status != "completed" && todo.status != "cancelled")
        .cloned()
        .collect();
    if !incomplete.is_empty() {
        return Some(RunAutoPokeFollowUp::Incomplete {
            count: incomplete.len(),
            message: build_run_poke_message(&incomplete),
        });
    }
    if !todos.is_empty()
        && let Some((message, confidence_spike_challenge)) =
            build_run_todo_validation_message(todos, !confidence_spike_challenged)
    {
        return Some(RunAutoPokeFollowUp::ConfidenceSummary {
            total_todos: todos.len(),
            message,
            confidence_spike_challenge,
        });
    }
    None
}

fn build_run_poke_message(incomplete: &[crate::todo::TodoItem]) -> String {
    crate::todo::build_auto_poke_message(incomplete.len())
}

fn build_run_todo_validation_message(
    todos: &[crate::todo::TodoItem],
    allow_confidence_spike_challenge: bool,
) -> Option<(String, bool)> {
    let completed: Vec<&crate::todo::TodoItem> = todos
        .iter()
        .filter(|todo| todo.status == "completed")
        .collect();
    if completed.is_empty() {
        return None;
    }

    let completion_confidence_needs_validation = completed.iter().any(|todo| {
        todo.completion_confidence
            .is_none_or(|score| score < RUN_TODO_CONFIDENCE_THRESHOLD)
    });
    let confidence_spike_detected =
        allow_confidence_spike_challenge && !crate::todo::spike_completed_todos(todos).is_empty();

    if !completion_confidence_needs_validation && !confidence_spike_detected {
        // Nothing actionable: completing the loop with a generic summary just
        // spends tokens on "all good" theater, so send nothing and end the run.
        return None;
    }

    if completion_confidence_needs_validation {
        crate::telemetry::record_todo_gate(crate::telemetry::TodoGateKind::Completion);
        Some((
            crate::todo::TODO_COMPLETION_CONTINUATION_MESSAGE.to_string(),
            false,
        ))
    } else {
        crate::telemetry::record_todo_gate(crate::telemetry::TodoGateKind::ConfidenceSpike);
        Some((
            crate::todo::TODO_CONFIDENCE_SPIKE_CONTINUATION_MESSAGE.to_string(),
            true,
        ))
    }
}

async fn run_single_message_command_plain_with_auto_poke(
    agent: &mut crate::agent::Agent,
    message: &str,
) -> Result<()> {
    let mut next_message = message.to_string();
    let max_turns = run_command_auto_poke_max_turns();
    let mut turns_completed = 0usize;
    let mut confidence_spike_challenged = false;
    loop {
        agent.run_once(&next_message).await?;
        turns_completed += 1;
        if !run_command_auto_poke_enabled() {
            break;
        }
        let todos = run_todos(agent.session_id());
        match build_run_auto_poke_follow_up_from_todos(&todos, confidence_spike_challenged) {
            Some(RunAutoPokeFollowUp::ConfidenceSummary {
                message,
                confidence_spike_challenge,
                ..
            }) => {
                if run_command_auto_poke_limit_reached(turns_completed, max_turns) {
                    if let Some(max_turns) = max_turns {
                        eprintln!(
                            "Auto-poke stopped after {max_turns} turn(s) with completion confidence still needing validation."
                        );
                    }
                    break;
                }
                confidence_spike_challenged |= confidence_spike_challenge;
                next_message = message;
                eprintln!(
                    "Auto-poking: todos complete; sending confidence summary follow-up. Set JCODE_RUN_AUTO_POKE=0 to disable."
                );
                continue;
            }
            Some(RunAutoPokeFollowUp::Incomplete { count, message }) => {
                if run_command_auto_poke_limit_reached(turns_completed, max_turns) {
                    if let Some(max_turns) = max_turns {
                        eprintln!(
                            "Auto-poke stopped after {max_turns} turn(s) with {} incomplete todo(s).",
                            count
                        );
                    }
                    break;
                }
                next_message = message;
                eprintln!(
                    "Auto-poking: {} incomplete todo(s). Set JCODE_RUN_AUTO_POKE=0 to disable.",
                    count
                );
            }
            None => break,
        }
    }
    Ok(())
}

async fn run_single_message_command_capture_with_auto_poke(
    agent: &mut crate::agent::Agent,
    message: &str,
) -> Result<String> {
    let mut next_message = message.to_string();
    let max_turns = run_command_auto_poke_max_turns();
    let mut outputs = Vec::new();
    let mut turns_completed = 0usize;
    let mut confidence_spike_challenged = false;
    loop {
        outputs.push(agent.run_once_capture(&next_message).await?);
        turns_completed += 1;
        if !run_command_auto_poke_enabled() {
            break;
        }
        let todos = run_todos(agent.session_id());
        match build_run_auto_poke_follow_up_from_todos(&todos, confidence_spike_challenged) {
            Some(RunAutoPokeFollowUp::ConfidenceSummary {
                message,
                confidence_spike_challenge,
                ..
            }) => {
                if run_command_auto_poke_limit_reached(turns_completed, max_turns) {
                    if let Some(max_turns) = max_turns {
                        outputs.push(format!(
                            "Auto-poke stopped after {max_turns} turn(s) with completion confidence still needing validation."
                        ));
                    }
                    break;
                }
                confidence_spike_challenged |= confidence_spike_challenge;
                next_message = message;
                continue;
            }
            Some(RunAutoPokeFollowUp::Incomplete { count, message }) => {
                if run_command_auto_poke_limit_reached(turns_completed, max_turns) {
                    if let Some(max_turns) = max_turns {
                        outputs.push(format!(
                            "Auto-poke stopped after {max_turns} turn(s) with {} incomplete todo(s).",
                            count
                        ));
                    }
                    break;
                }
                next_message = message;
            }
            None => break,
        }
    }
    Ok(outputs.join("\n\n"))
}

fn restore_agent_session_if_requested(
    agent: &mut crate::agent::Agent,
    resume_session: Option<&str>,
) -> Result<()> {
    if let Some(session_id) = resume_session {
        agent.restore_session(session_id)?;
    }
    Ok(())
}

async fn run_single_message_command_ndjson(
    agent: &mut crate::agent::Agent,
    provider: std::sync::Arc<dyn crate::provider::Provider>,
    message: &str,
) -> Result<()> {
    let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();
    let session_id = agent.session_id().to_string();
    let mut stdout = std::io::stdout().lock();
    let mut state = NdjsonRunState {
        session_id: Some(session_id.clone()),
        ..NdjsonRunState::default()
    };
    write_json_line(
        &mut stdout,
        &serde_json::json!({
            "type": "start",
            "session_id": session_id,
            "provider": provider.name(),
            "model": provider.model(),
        }),
    )?;

    let max_turns = run_command_auto_poke_max_turns();
    let mut next_message = message.to_string();
    let mut result: Result<()> = Ok(());
    let mut turns_completed = 0usize;
    let mut confidence_spike_challenged = false;
    loop {
        let turn_result = {
            let mut run_future = std::pin::pin!(agent.run_once_streaming_mpsc(
                &next_message,
                Vec::new(),
                None,
                event_tx.clone(),
            ));
            let mut run_result: Option<Result<()>> = None;
            loop {
                tokio::select! {
                    result = &mut run_future, if run_result.is_none() => {
                        run_result = Some(result);
                    }
                    event = event_rx.recv() => {
                        match event {
                            Some(event) => emit_ndjson_event(&mut stdout, &mut state, event)?,
                            None => break,
                        }
                    }
                }
                if run_result.is_some() {
                    while let Ok(event) = event_rx.try_recv() {
                        emit_ndjson_event(&mut stdout, &mut state, event)?;
                    }
                    break;
                }
            }
            run_result.unwrap_or(Ok(()))
        };

        if let Err(err) = turn_result {
            result = Err(err);
            break;
        }
        turns_completed += 1;
        if !run_command_auto_poke_enabled() {
            break;
        }
        let todos = run_todos(&session_id);
        match build_run_auto_poke_follow_up_from_todos(&todos, confidence_spike_challenged) {
            Some(RunAutoPokeFollowUp::ConfidenceSummary {
                total_todos,
                message,
                confidence_spike_challenge,
            }) => {
                if run_command_auto_poke_limit_reached(turns_completed, max_turns) {
                    if let Some(max_turns) = max_turns {
                        write_json_line(
                            &mut stdout,
                            &serde_json::json!({
                                "type": "auto_poke_stopped",
                                "session_id": session_id,
                                "completion_confidence_needs_validation": true,
                                "max_turns": max_turns,
                            }),
                        )?;
                    }
                    break;
                }
                confidence_spike_challenged |= confidence_spike_challenge;
                next_message = message;
                write_json_line(
                    &mut stdout,
                    &serde_json::json!({
                        "type": "auto_poke_confidence_summary",
                        "session_id": session_id,
                        "todos": total_todos,
                        "confidence_spike_challenge": confidence_spike_challenge,
                        "message": next_message,
                    }),
                )?;
                continue;
            }
            Some(RunAutoPokeFollowUp::Incomplete { count, message }) => {
                if run_command_auto_poke_limit_reached(turns_completed, max_turns) {
                    if let Some(max_turns) = max_turns {
                        write_json_line(
                            &mut stdout,
                            &serde_json::json!({
                                "type": "auto_poke_stopped",
                                "session_id": session_id,
                                "incomplete_todos": count,
                                "max_turns": max_turns,
                            }),
                        )?;
                    }
                    break;
                }
                next_message = message;
                write_json_line(
                    &mut stdout,
                    &serde_json::json!({
                        "type": "auto_poke",
                        "session_id": session_id,
                        "incomplete_todos": count,
                        "message": next_message,
                    }),
                )?;
            }
            None => break,
        }
    }

    match result {
        Ok(()) => {
            write_json_line(
                &mut stdout,
                &serde_json::json!({
                    "type": "done",
                    "session_id": session_id,
                    "provider": provider.name(),
                    "model": provider.model(),
                    "text": state.text,
                    "usage": state.usage,
                    "upstream_provider": state.upstream_provider,
                    "connection_type": state.connection_type,
                    "connection_phase": state.connection_phase,
                    "status_detail": state.status_detail,
                }),
            )?;
            Ok(())
        }
        Err(err) => {
            write_json_line(
                &mut stdout,
                &serde_json::json!({
                    "type": "error",
                    "session_id": session_id,
                    "provider": provider.name(),
                    "model": provider.model(),
                    "message": format!("{err:#}"),
                }),
            )?;
            Err(err)
        }
    }
}

fn emit_ndjson_event(
    stdout: &mut impl Write,
    state: &mut NdjsonRunState,
    event: crate::protocol::ServerEvent,
) -> Result<()> {
    use crate::protocol::ServerEvent;

    match event {
        ServerEvent::TextDelta { text } => {
            state.text.push_str(&text);
            write_json_line(
                stdout,
                &serde_json::json!({ "type": "text_delta", "text": text }),
            )
        }
        ServerEvent::TextReplace { text } => {
            state.text = text.clone();
            write_json_line(
                stdout,
                &serde_json::json!({ "type": "text_replace", "text": text }),
            )
        }
        ServerEvent::ToolStart { id, name } => write_json_line(
            stdout,
            &serde_json::json!({ "type": "tool_start", "id": id, "name": name }),
        ),
        ServerEvent::ToolInput { delta } => write_json_line(
            stdout,
            &serde_json::json!({ "type": "tool_input", "delta": delta }),
        ),
        ServerEvent::ToolExec { id, name } => write_json_line(
            stdout,
            &serde_json::json!({ "type": "tool_exec", "id": id, "name": name }),
        ),
        ServerEvent::ToolDone {
            id,
            name,
            output,
            error,
        } => write_json_line(
            stdout,
            &serde_json::json!({
                "type": "tool_done",
                "id": id,
                "name": name,
                "output": output,
                "error": error,
            }),
        ),
        ServerEvent::TokenUsage {
            input,
            output,
            cache_read_input,
            cache_creation_input,
        } => {
            state.usage = crate::agent::TokenUsage {
                input_tokens: input,
                output_tokens: output,
                cache_read_input_tokens: cache_read_input,
                cache_creation_input_tokens: cache_creation_input,
            };
            write_json_line(
                stdout,
                &serde_json::json!({
                    "type": "tokens",
                    "input": input,
                    "output": output,
                    "cache_read_input": cache_read_input,
                    "cache_creation_input": cache_creation_input,
                }),
            )
        }
        ServerEvent::ConnectionType { connection } => {
            state.connection_type = Some(connection.clone());
            write_json_line(
                stdout,
                &serde_json::json!({ "type": "connection_type", "connection": connection }),
            )
        }
        ServerEvent::ConnectionPhase { phase } => {
            state.connection_phase = Some(phase.clone());
            write_json_line(
                stdout,
                &serde_json::json!({ "type": "connection_phase", "phase": phase }),
            )
        }
        ServerEvent::StatusDetail { detail } => {
            state.status_detail = Some(detail.clone());
            write_json_line(
                stdout,
                &serde_json::json!({ "type": "status_detail", "detail": detail }),
            )
        }
        ServerEvent::MessageEnd => {
            write_json_line(stdout, &serde_json::json!({ "type": "message_end" }))
        }
        ServerEvent::UpstreamProvider { provider } => {
            state.upstream_provider = Some(provider.clone());
            write_json_line(
                stdout,
                &serde_json::json!({ "type": "upstream_provider", "provider": provider }),
            )
        }
        ServerEvent::SessionId { session_id } => {
            state.session_id = Some(session_id.clone());
            write_json_line(
                stdout,
                &serde_json::json!({ "type": "session", "session_id": session_id }),
            )
        }
        ServerEvent::Compaction {
            trigger,
            pre_tokens,
            messages_dropped,
            post_tokens,
            tokens_saved,
            duration_ms,
            messages_compacted,
            summary_chars,
            active_messages,
        } => write_json_line(
            stdout,
            &serde_json::json!({
                "type": "compaction",
                "trigger": trigger,
                "pre_tokens": pre_tokens,
                "messages_dropped": messages_dropped,
                "post_tokens": post_tokens,
                "tokens_saved": tokens_saved,
                "duration_ms": duration_ms,
                "messages_compacted": messages_compacted,
                "summary_chars": summary_chars,
                "active_messages": active_messages,
            }),
        ),
        ServerEvent::MemoryInjected {
            count,
            prompt_chars,
            computed_age_ms,
            ..
        } => write_json_line(
            stdout,
            &serde_json::json!({
                "type": "memory_injected",
                "count": count,
                "prompt_chars": prompt_chars,
                "computed_age_ms": computed_age_ms,
            }),
        ),
        ServerEvent::Interrupted => {
            write_json_line(stdout, &serde_json::json!({ "type": "interrupted" }))
        }
        ServerEvent::SoftInterruptInjected {
            content,
            display_role,
            point,
            tools_skipped,
        } => write_json_line(
            stdout,
            &serde_json::json!({
                "type": "soft_interrupt_injected",
                "content": content,
                "display_role": display_role,
                "point": point,
                "tools_skipped": tools_skipped,
            }),
        ),
        ServerEvent::BatchProgress { progress } => write_json_line(
            stdout,
            &serde_json::json!({ "type": "batch_progress", "progress": progress }),
        ),
        ServerEvent::Error {
            message,
            retry_after_secs,
            ..
        } => write_json_line(
            stdout,
            &serde_json::json!({
                "type": "error",
                "message": message,
                "retry_after_secs": retry_after_secs,
            }),
        ),
        ServerEvent::Ack { .. } | ServerEvent::Done { .. } | ServerEvent::Pong { .. } => Ok(()),
        _ => Ok(()),
    }
}

fn write_json_line(stdout: &mut impl Write, value: &impl Serialize) -> Result<()> {
    serde_json::to_writer(&mut *stdout, value)?;
    stdout.write_all(b"\n")?;
    stdout.flush()?;
    Ok(())
}

pub async fn run_model_command(
    choice: &super::provider_init::ProviderChoice,
    model: Option<&str>,
    emit_json: bool,
    verbose: bool,
) -> Result<()> {
    let provider = super::provider_init::init_provider_quiet(choice, model).await?;

    if let Err(err) = provider.prefetch_models().await
        && !super::output::quiet_enabled()
    {
        eprintln!("Warning: failed to refresh dynamic model list: {}", err);
    }

    let routes = provider.model_routes();
    let filtered_routes = filter_cli_model_routes_for_choice(choice, &routes);
    let models = if filtered_routes.len() == routes.len() {
        collect_cli_model_names(&routes, provider.available_models_display())
    } else {
        collect_cli_model_names(&filtered_routes, Vec::new())
    };

    if models.is_empty() {
        anyhow::bail!(
            "No models found for provider '{}'. Check credentials or try a different --provider.",
            provider.name()
        );
    }

    if emit_json {
        let provider_label = super::provider_init::login_provider_for_choice(choice)
            .map(|provider| provider.display_name.to_string())
            .unwrap_or_else(|| {
                crate::provider_catalog::runtime_provider_display_name(provider.name())
            });
        let report = ModelListReport {
            provider: provider_label,
            selected_model: provider.model(),
            models,
            routes: filtered_routes
                .iter()
                .map(|route| ModelListRouteReport {
                    provider: cli_route_provider_display(&route.provider, &route.api_method),
                    model: route.model.clone(),
                    method: cli_api_method_display(&route.api_method),
                    available: route.available,
                })
                .collect(),
        };
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        if verbose {
            println!(
                "Provider: {}",
                crate::provider_catalog::runtime_provider_display_name(provider.name())
            );
            println!("Selected model: {}", provider.model());
            println!("Available models: {}", models.len());
            println!();
        }
        for model in models {
            println!("{}", model);
        }
    }

    Ok(())
}

fn cli_api_method_display(raw: &str) -> String {
    crate::provider::ModelRouteApiMethod::parse(raw).display_label()
}

fn cli_route_provider_display(provider: &str, api_method: &str) -> String {
    if crate::provider::ModelRouteApiMethod::parse(api_method).is_openrouter()
        && provider != "auto"
        && !provider.contains("OpenRouter")
    {
        format!("OpenRouter/{}", provider)
    } else {
        provider.to_string()
    }
}

fn collect_cli_model_names(
    routes: &[crate::provider::ModelRoute],
    display_models: Vec<String>,
) -> Vec<String> {
    let mut deduped = Vec::new();
    let mut seen = BTreeSet::new();

    fn push_model(deduped: &mut Vec<String>, seen: &mut BTreeSet<String>, model: &str) {
        let trimmed = model.trim();
        if !crate::provider::is_listable_model_name(trimmed) {
            return;
        }
        if seen.insert(trimmed.to_string()) {
            deduped.push(trimmed.to_string());
        }
    }

    for route in routes.iter().filter(|route| route.available) {
        push_model(&mut deduped, &mut seen, &route.model);
    }

    if deduped.is_empty() {
        for route in routes {
            push_model(&mut deduped, &mut seen, &route.model);
        }
    }

    for model in display_models {
        push_model(&mut deduped, &mut seen, &model);
    }

    deduped
}

#[allow(deprecated)]
fn filter_cli_model_routes_for_choice(
    choice: &super::provider_init::ProviderChoice,
    routes: &[crate::provider::ModelRoute],
) -> Vec<crate::provider::ModelRoute> {
    use super::provider_init::ProviderChoice;

    let keep = |route: &&crate::provider::ModelRoute| match choice {
        ProviderChoice::Claude | ProviderChoice::ClaudeSubprocess => {
            route.api_method_kind().is_anthropic_credential_route()
        }
        ProviderChoice::Openai => {
            let method = route.api_method_kind();
            matches!(method, crate::provider::ModelRouteApiMethod::OpenAIOAuth)
                || matches!(method, crate::provider::ModelRouteApiMethod::Other(ref value) if value == "chatgpt-web")
        }
        ProviderChoice::OpenaiApi => matches!(
            route.api_method_kind(),
            crate::provider::ModelRouteApiMethod::OpenAIApiKey
        ),
        ProviderChoice::Openrouter | ProviderChoice::Azure => {
            route.api_method_kind().is_openrouter()
        }
        ProviderChoice::Copilot => route.api_method_kind().is_copilot(),
        _ => true,
    };

    let filtered: Vec<_> = routes.iter().filter(keep).cloned().collect();
    if filtered.is_empty() {
        routes.to_vec()
    } else {
        filtered
    }
}
#[cfg(test)]
#[path = "commands_tests.rs"]
mod tests;
