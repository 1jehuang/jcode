//! Discover mobile-created sessions without copying their transcripts locally.
use super::super::cloud_preview::StageTimer;
use super::*;

#[derive(Deserialize)]
struct VerifiedInventory {
    host: String,
    sessions: Vec<VerifiedSession>,
}

#[derive(Deserialize)]
struct VerifiedSession {
    session_id: String,
    epoch: u64,
    cwd: Option<String>,
    title: Option<String>,
    updated_at: Option<String>,
}

/// Called only after publishing local results, never on the render thread.
pub(super) fn available() -> bool {
    crate::mobile_sync::session_source() != "cloud"
        && crate::subscription_catalog::configured_api_key()
            .is_some_and(|key| !key.trim().is_empty())
}

pub(super) fn refresh() -> Result<bool> {
    let mut timer = StageTimer::new("inventory_refresh");
    if !available() {
        timer.outcome("skipped_unavailable");
        return Ok(false);
    }
    let home = storage::jcode_dir()?;
    let config = home.join("cloud/ssh_config");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let result = runtime.block_on(async {
        let mut discovery = StageTimer::new("inventory_discovery");
        let inventory = crate::mobile_sync::discover_cloud_sessions().await?;
        discovery.outcome("success");
        drop(discovery);
        let candidates: Vec<_> = inventory
            .into_iter()
            .filter(|s| should_verify(&home, &s.session_id))
            .collect();
        if candidates.is_empty() {
            return Ok(false);
        }
        let mut prepare = StageTimer::new("inventory_prepare_ssh");
        prepare_ssh_config(&config).await?;
        prepare.outcome("success");
        drop(prepare);
        let ids = candidates
            .iter()
            .map(|s| s.session_id.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let command = crate::tui::cloud_git::compressed_script_command(
            Some("jcode-cloud"),
            Some(&config),
            &ids,
            &super::super::cloud_preview::projection_script("inventory"),
        );
        let mut fetch = StageTimer::new("inventory_ssh");
        let (status, bytes) = crate::tui::cloud_git::bounded_output_for(
            command,
            Duration::from_secs(10),
            1024 * 1024,
            crate::tui::cloud_git::WorkspaceOperation::InventoryProjection,
        )
        .await
        .ok_or_else(|| anyhow::anyhow!("cloud inventory verification unavailable"))?;
        anyhow::ensure!(status.success(), "cloud inventory verification failed");
        fetch.outcome("success");
        drop(fetch);
        let mut parse = StageTimer::new("inventory_parse");
        let verified: VerifiedInventory = serde_json::from_slice(&bytes)?;
        parse.outcome("success");
        drop(parse);
        let mut registration = StageTimer::new("inventory_register");
        let result = register(&home, candidates, verified);
        if result.is_ok() {
            registration.outcome("success");
        }
        result
    });
    if result.is_ok() {
        timer.outcome("success");
    }
    result
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
}

fn should_verify(home: &Path, id: &str) -> bool {
    if !valid_id(id) {
        return false;
    }
    match storage::read_session_lease(id) {
        Some(lease) => lease.session_id == id && lease.away_host.as_deref() == Some("jcode-cloud"),
        None => {
            !home
                .join("session_leases")
                .join(format!("{id}.json"))
                .exists()
                && !home.join("sessions").join(format!("{id}.json")).exists()
        }
    }
}

async fn prepare_ssh_config(config: &Path) -> Result<()> {
    let mut command = tokio::process::Command::new(std::env::current_exe()?);
    command
        .args([
            "--no-update",
            "--no-selfdev",
            "cloud",
            "machine",
            "prepare-ssh",
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let (status, _) = crate::tui::cloud_git::bounded_output_for(
        command,
        Duration::from_secs(8),
        4096,
        crate::tui::cloud_git::WorkspaceOperation::InventoryPrepare,
    )
    .await
    .ok_or_else(|| anyhow::anyhow!("cloud SSH setup unavailable"))?;
    anyhow::ensure!(
        status.success() && config.is_file(),
        "cloud SSH setup failed"
    );
    Ok(())
}

fn register(
    home: &Path,
    candidates: Vec<crate::mobile_sync::SyncedSession>,
    verified: VerifiedInventory,
) -> Result<bool> {
    let mut changed = false;
    for remote in verified.sessions {
        let Some(metadata) = candidates.iter().find(|s| {
            s.session_id == remote.session_id && s.host.as_deref() == Some(verified.host.as_str())
        }) else {
            continue;
        };
        if !valid_id(&remote.session_id) || !should_verify(home, &remote.session_id) {
            continue;
        }
        // Never let metadata discovery supersede a migration. Only update an
        // existing managed-cloud lease at the exact epoch we verified remotely.
        let existing = storage::read_session_lease(&remote.session_id);
        if existing
            .as_ref()
            .is_some_and(|lease| lease.epoch != remote.epoch)
        {
            continue;
        }
        let mut lease = existing.clone().unwrap_or_else(|| storage::SessionLease {
            session_id: remote.session_id.clone(),
            epoch: remote.epoch,
            away_host: Some("jcode-cloud".into()),
            repo_root: None,
            last_sync_snapshot: None,
            last_sync_head: None,
            updated_at: String::new(),
        });
        lease.repo_root = remote.cwd;
        if let Some(updated_at) = remote.updated_at.or_else(|| metadata.updated_at.clone()) {
            // Ignore malformed timestamps rather than poisoning picker sorting.
            if chrono::DateTime::parse_from_rfc3339(&updated_at).is_ok() {
                lease.updated_at = updated_at;
            }
        }
        if lease.updated_at.is_empty() {
            lease.updated_at = chrono::Utc::now().to_rfc3339();
        }
        if existing.as_ref() != Some(&lease) {
            storage::write_session_lease(&lease)?;
            changed = true;
        }
        // A picker-only sidecar preserves metadata, never a canonical transcript.
        let path = home
            .join("cloud/mobile-session-titles")
            .join(format!("{}.json", lease.session_id));
        let title = remote.title.unwrap_or_else(|| {
            let name = extract_session_name(&lease.session_id).unwrap_or(&lease.session_id);
            format!("{name} (cloud session)")
        });
        if storage::read_json::<String>(&path).ok().as_deref() != Some(&title) {
            storage::write_json_secret(&path, &title)?;
            changed = true;
        }
    }
    Ok(changed)
}

pub(super) fn title(id: &str) -> Option<String> {
    let path = storage::jcode_dir()
        .ok()?
        .join("cloud/mobile-session-titles")
        .join(format!("{id}.json"));
    storage::read_json(&path).ok()
}

#[cfg(test)]
#[path = "mobile_inventory_tests.rs"]
mod tests;
