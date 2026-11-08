//! Discover mobile-created sessions without copying their transcripts locally.
use super::*;

const VERIFY_SCRIPT: &str = r#"
python3 - "$1" <<'PY'
import json, os, socket, sys
from pathlib import Path
base = Path(os.environ.get('JCODE_HOME', str(Path.home() / '.jcode')))
result = []
for sid in sys.argv[1].splitlines():
    if not sid or not all(c.isascii() and (c.isalnum() or c in '_-') for c in sid):
        continue
    try:
        session = json.loads((base / 'sessions' / (sid + '.json')).read_text())
        if session.get('id') != sid:
            continue
        lease_path = base / 'session_leases' / (sid + '.json')
        lease = json.loads(lease_path.read_text()) if lease_path.exists() else {}
        if lease.get('away_host'):
            continue
        result.append({'session_id': sid, 'epoch': max(session.get('migration_epoch', 0), lease.get('epoch', 0)), 'cwd': session.get('working_dir')})
    except (OSError, ValueError, TypeError):
        continue
print(json.dumps({'host': socket.gethostname(), 'sessions': result}))
PY
"#;

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
}

/// Called only after publishing local results, never on the render thread.
pub(super) fn available() -> bool {
    crate::mobile_sync::session_source() != "cloud"
        && crate::subscription_catalog::has_credentials()
        && storage::jcode_dir().is_ok_and(|home| home.join("cloud/ssh_config").is_file())
}

pub(super) fn refresh() -> Result<bool> {
    if !available() {
        return Ok(false);
    }
    let home = storage::jcode_dir()?;
    let config = home.join("cloud/ssh_config");
    if !config.is_file() {
        return Ok(false);
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let inventory = crate::mobile_sync::discover_cloud_sessions().await?;
        let candidates: Vec<_> = inventory
            .into_iter()
            .filter(|s| {
                storage::read_session_lease(&s.session_id).is_none()
                    && !home
                        .join("sessions")
                        .join(format!("{}.json", s.session_id))
                        .exists()
            })
            .collect();
        if candidates.is_empty() {
            return Ok(false);
        }
        let ids = candidates
            .iter()
            .map(|s| s.session_id.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let command = crate::tui::cloud_git::compressed_script_command(
            Some("jcode-cloud"),
            Some(&config),
            &ids,
            VERIFY_SCRIPT,
        );
        let (status, bytes) =
            crate::tui::cloud_git::bounded_output(command, Duration::from_secs(10), 1024 * 1024)
                .await
                .ok_or_else(|| anyhow::anyhow!("cloud inventory verification unavailable"))?;
        anyhow::ensure!(status.success(), "cloud inventory verification failed");
        let verified: VerifiedInventory = serde_json::from_slice(&bytes)?;
        register(&home, candidates, verified)
    })
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
        // Recheck after network IO. Never replace a migration decision or local work.
        if storage::read_session_lease(&remote.session_id).is_some()
            || home
                .join("sessions")
                .join(format!("{}.json", remote.session_id))
                .exists()
        {
            continue;
        }
        let lease = storage::SessionLease {
            session_id: remote.session_id,
            epoch: remote.epoch,
            away_host: Some("jcode-cloud".into()),
            repo_root: remote.cwd.or_else(|| metadata.cwd.clone()),
            last_sync_snapshot: None,
            last_sync_head: None,
            updated_at: metadata
                .updated_at
                .clone()
                .unwrap_or_else(|| chrono::Utc::now().to_rfc3339()),
        };
        storage::write_session_lease(&lease)?;
        // A picker-only sidecar preserves the title, not a canonical transcript.
        let path = home
            .join("cloud/mobile-session-titles")
            .join(format!("{}.json", lease.session_id));
        if let Some(title) = metadata.title.as_deref() {
            storage::write_json(&path, &title)?;
        }
        changed = true;
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
