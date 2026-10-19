//! Packaged local upgrades deliberately do not use the interrupting reload path.
use crate::protocol::ServerEvent;
use crate::runtime_activity::ActivityGate;
use anyhow::{Context, Result, bail, ensure};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::sync::{Mutex, OwnedRwLockWriteGuard};

const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_REPORT_BYTES: u64 = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
struct FileIdentity {
    len: u64,
    modified: std::time::SystemTime,
    #[cfg(unix)]
    dev_inode: (u64, u64),
}

fn identity(path: &Path) -> Result<FileIdentity> {
    let meta = std::fs::metadata(path)?;
    ensure!(meta.is_file(), "candidate is not a regular file");
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // This is a local, same-user operation, not an arbitrary shell command.
        let uid = unsafe { libc::geteuid() };
        ensure!(
            meta.uid() == uid || meta.uid() == 0,
            "candidate has a different owner"
        );
        ensure!(
            meta.mode() & 0o022 == 0,
            "candidate is group/world writable"
        );
        ensure!(meta.mode() & 0o111 != 0, "candidate is not executable");
    }
    Ok(FileIdentity {
        len: meta.len(),
        modified: meta.modified()?,
        #[cfg(unix)]
        dev_inode: {
            use std::os::unix::fs::MetadataExt;
            (meta.dev(), meta.ino())
        },
    })
}

fn validate_path(candidate: &Path) -> Result<(PathBuf, FileIdentity)> {
    use std::io::Read;
    ensure!(
        candidate.is_absolute(),
        "candidate must be an explicit absolute local path"
    );
    let path = candidate
        .canonicalize()
        .context("candidate does not exist")?;
    let metadata = identity(&path)?;
    // Never interpret a script, launcher, URL or directory supplied as a binary.
    let mut magic = [0_u8; 4];
    std::fs::File::open(&path)?.read_exact(&mut magic)?;
    ensure!(
        matches!(
            magic,
            [0x7f, b'E', b'L', b'F']
                | [0xcf, 0xfa, 0xed, 0xfe]
                | [0xfe, 0xed, 0xfa, 0xcf]
                | [0xca, 0xfe, 0xba, 0xbe]
                | [0xbe, 0xba, 0xfe, 0xca]
        ),
        "candidate is not a supported native executable"
    );
    Ok((path, metadata))
}

fn strict_version(value: &str) -> Result<(u64, u64, u64)> {
    let mut parts = value.trim_start_matches('v').split('.');
    let major = parts.next().context("missing version major")?.parse()?;
    let minor = parts.next().context("missing version minor")?.parse()?;
    let patch = parts.next().context("missing version patch")?.parse()?;
    ensure!(parts.next().is_none(), "invalid version");
    Ok((major, minor, patch))
}

fn remove_runtime_identity(command: &mut std::process::Command) {
    for key in [
        "JCODE_RUNTIME_RELEASE_SEMVER",
        "JCODE_RUNTIME_RELEASE_GIT_HASH",
        "JCODE_RUNTIME_RELEASE_GIT_DATE",
        "JCODE_RUNTIME_RELEASE_GIT_TAG",
        "JCODE_READY_FD",
    ] {
        command.env_remove(key);
    }
}

async fn probe_version(path: &Path) -> Result<String> {
    let mut command = std::process::Command::new(path);
    remove_runtime_identity(&mut command);
    command
        .args(["--no-update", "version", "--json"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut command = tokio::process::Command::from(command);
    command.kill_on_drop(true);
    let mut child = command.spawn().context("could not probe candidate")?;
    let mut stdout = child
        .stdout
        .take()
        .context("candidate stdout unavailable")?
        .take(MAX_REPORT_BYTES + 1);
    let result = tokio::time::timeout(PROBE_TIMEOUT, async {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).await?;
        ensure!(
            bytes.len() as u64 <= MAX_REPORT_BYTES,
            "candidate version report too large"
        );
        ensure!(
            child.wait().await?.success(),
            "candidate version probe failed"
        );
        let report: serde_json::Value =
            serde_json::from_slice(&bytes).context("not a jcode version report")?;
        ensure!(
            report.get("git_hash").and_then(|v| v.as_str()).is_some(),
            "not a jcode version report"
        );
        Ok::<_, anyhow::Error>(
            report
                .get("update_semver")
                .and_then(|v| v.as_str())
                .context("candidate has no update version")?
                .to_string(),
        )
    })
    .await;
    match result {
        Ok(Ok(version)) => Ok(version),
        other => {
            let _ = child.kill().await;
            match other {
                Ok(Err(error)) => Err(error),
                _ => bail!("candidate version probe timed out"),
            }
        }
    }
}

struct PreparedUpgrade {
    path: PathBuf,
    identity: FileIdentity,
    _idle: OwnedRwLockWriteGuard<()>,
}

enum UpgradeDecision {
    Ready(PreparedUpgrade),
    Unchanged,
    Busy,
}

fn reserve_upgrade(
    path: PathBuf,
    metadata: FileIdentity,
    version: &str,
    current: &str,
    gate: &ActivityGate,
) -> Result<UpgradeDecision> {
    if strict_version(version)? <= strict_version(current)? {
        return Ok(UpgradeDecision::Unchanged);
    }
    let Some(idle) = gate.try_idle() else {
        return Ok(UpgradeDecision::Busy);
    };
    ensure!(
        identity(&path)? == metadata,
        "candidate changed during validation"
    );
    Ok(UpgradeDecision::Ready(PreparedUpgrade {
        path,
        identity: metadata,
        _idle: idle,
    }))
}

async fn prepare_upgrade(candidate: &Path) -> Result<UpgradeDecision> {
    let (path, metadata) = validate_path(candidate)?;
    let version = probe_version(&path).await?;
    reserve_upgrade(
        path,
        metadata,
        &version,
        jcode_build_meta::update_semver(),
        crate::runtime_activity::global(),
    )
}

fn status(_id: u64, step: &str, message: impl Into<String>) -> ServerEvent {
    ServerEvent::ReloadProgress {
        step: step.into(),
        message: message.into(),
        success: Some(step != "invalid" && step != "failed"),
        output: None,
    }
}

/// Called before session initialization. No agent or provider is constructed for
/// the probe, including old clients that merely want to upgrade a local daemon.
pub(super) async fn handle(
    id: u64,
    candidate: &str,
    writer: &Arc<Mutex<crate::transport::WriteHalf>>,
    sessions: &super::SessionAgents,
) -> Result<()> {
    use super::client_writer::write_direct_event;
    #[cfg(not(unix))]
    {
        let _ = (candidate, sessions);
        return write_direct_event(
            writer,
            &status(
                id,
                "unsupported",
                "Safe packaged runtime upgrades are not available on this platform.",
            ),
        )
        .await;
    }
    #[cfg(unix)]
    {
        let prepared = match prepare_upgrade(Path::new(candidate)).await {
            Ok(UpgradeDecision::Ready(prepared)) => prepared,
            Ok(UpgradeDecision::Unchanged) => return write_direct_event(writer, &status(id, "unchanged", "Runtime is already as new as the bundled executable. No downgrade or reload performed.")).await,
            Ok(UpgradeDecision::Busy) => return write_direct_event(writer, &status(id, "busy", "Runtime upgrade deferred: active turns or background tools are still running. Existing work was not interrupted.")).await,
            Err(error) => return write_direct_event(writer, &status(id, "invalid", format!("Runtime upgrade deferred: {error:#}"))).await,
        };
        // Freeze the session index and reserve every idle agent until exec.
        // This also catches autonomous/session-control work outside a model
        // turn. Never wait for an agent lock or checkpoint an active agent.
        let delivery_checkpoint = match crate::bus::Bus::global().runtime_delivery_checkpoint() {
            Some(checkpoint) => checkpoint,
            None => {
                drop(prepared);
                return write_direct_event(writer, &status(id, "busy", "Runtime upgrade deferred: completion delivery is pending. Existing work was not interrupted.")).await;
            }
        };
        let checkpoint = checkpoint_sessions(sessions);
        let (_session_index, _idle_agents) = match checkpoint {
            Ok(checkpoint) => checkpoint,
            Err(error) => {
                drop(prepared);
                return write_direct_event(
                    writer,
                    &status(
                        id,
                        "busy",
                        format!("Runtime upgrade deferred without interrupting work: {error:#}"),
                    ),
                )
                .await;
            }
        };
        ensure!(
            identity(&prepared.path)? == prepared.identity,
            "candidate changed before exec"
        );
        // Bound the exclusive interval even if the requesting client stops reading.
        tokio::time::timeout(
            Duration::from_millis(500),
            write_direct_event(
                writer,
                &status(
                    id,
                    "upgrading",
                    "Applying newer bundled runtime while idle. Reconnecting...",
                ),
            ),
        )
        .await??;
        let request_id = crate::id::new_id("packaged_upgrade");
        super::write_reload_state(&request_id, "packaged", super::ReloadPhase::Starting, None);
        let mut command = std::process::Command::new(&prepared.path);
        command
            .args(["--no-update", "serve", "--socket"])
            .arg(super::socket_path());
        remove_runtime_identity(&mut command);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // Do not unlink sockets first. CLOEXEC closes their listeners on success
        // and the new server removes stale paths. On exec failure the old runtime
        // remains reachable. No checkpoint signals or task aborts are used here.
        let replacement =
            replace_after_delivery_checkpoint(&mut command, delivery_checkpoint, &prepared);
        // No deferred or failed response may retain the exclusive activity,
        // session-index, or agent reservations while waiting for a client.
        drop(_idle_agents);
        drop(_session_index);
        drop(prepared);
        let error = match replacement {
            Ok(error) => error,
            Err(error) => {
                super::write_reload_state(
                    &request_id,
                    "packaged",
                    super::ReloadPhase::Failed,
                    Some(error.to_string()),
                );
                return write_direct_event(writer, &status(id, "busy", format!("Runtime upgrade deferred without interrupting completion delivery: {error}"))).await;
            }
        };
        super::write_reload_state(
            &request_id,
            "packaged",
            super::ReloadPhase::Failed,
            Some(error.to_string()),
        );
        write_direct_event(
            writer,
            &status(
                id,
                "failed",
                format!("Runtime upgrade failed; existing runtime retained: {error}"),
            ),
        )
        .await
    }
}

/// This fence is deliberately synchronous: no lock is retained over a reply,
/// checkpoint, logging call, or other operation that could publish a bus event.
#[cfg(unix)]
fn replace_after_delivery_checkpoint(
    command: &mut std::process::Command,
    checkpoint: usize,
    prepared: &PreparedUpgrade,
) -> Result<std::io::Error> {
    ensure!(
        identity(&prepared.path)? == prepared.identity,
        "candidate changed while notifying the client"
    );
    let _publication = crate::bus::Bus::global()
        .try_runtime_delivery_idle(checkpoint)
        .context("completion events are still queued or being delivered")?;
    // The shared replace_process helper logs on failure. Logging hooks may
    // publish bus events, so use raw exec while holding the publication fence.
    use std::os::unix::process::CommandExt;
    Ok(command.exec())
}

type SessionIndex = std::collections::HashMap<String, Arc<Mutex<crate::agent::Agent>>>;

fn checkpoint_sessions(
    sessions: &super::SessionAgents,
) -> Result<(
    tokio::sync::RwLockReadGuard<'_, SessionIndex>,
    Vec<tokio::sync::OwnedMutexGuard<crate::agent::Agent>>,
)> {
    ensure!(
        !crate::tool::sdk::has_runtime_overlays(),
        "session-local SDK tool restrictions/callbacks must be reconfigured before an automatic upgrade can proceed"
    );
    let index = sessions.try_read().context("session index is changing")?;
    let mut agents = Vec::with_capacity(index.len());
    for agent in index.values() {
        agents.push(
            agent
                .clone()
                .try_lock_owned()
                .map_err(|_| anyhow::anyhow!("a session is busy"))?,
        );
    }
    let started = std::time::Instant::now();
    for agent in &mut agents {
        agent.checkpoint_idle_runtime_upgrade()?;
        ensure!(
            started.elapsed() < Duration::from_secs(1),
            "session checkpoint budget exhausted; retry later"
        );
    }
    Ok((index, agents))
}

#[cfg(all(test, unix))]
#[path = "runtime_upgrade_tests.rs"]
mod tests;
