//! Short-lived local resource callback for an explicitly migrated cloud session.
//! Only the CLI handles resource bytes. The model sees a closed status vocabulary.
use super::App;
use crate::protocol::{SessionToolConfig, SessionToolDefinition};
use crate::tui::backend::RemoteConnection;
use anyhow::Context;
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Default)]
pub(super) struct CloudEnvironment {
    grant: Option<Grant>,
    // Kept across reconnects. Re-reading an updated lease cannot extend access.
    deadline: Option<tokio::time::Instant>,
    switched_away: bool,
}
struct Grant {
    session: String,
    deadline: tokio::time::Instant,
    live: Arc<AtomicBool>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
    // The timer revokes locally without sending a control request mid-turn.
    // Keep one idle cleanup due independently of the local live flag.
    clear_after_idle: bool,
}
impl Grant {
    fn allows(&self, expected: Option<&str>, callback: &str) -> bool {
        expected == Some(callback)
            && self.session == callback
            && self.live.load(Ordering::SeqCst)
            && tokio::time::Instant::now() < self.deadline
    }
}
impl Drop for Grant {
    fn drop(&mut self) {
        self.live.store(false, Ordering::SeqCst);
        for task in &self.tasks {
            task.abort();
        }
    }
}

fn lease_remaining(
    lease: &crate::storage::SessionLease,
    session: &str,
    marker: &str,
    host: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<Duration> {
    if session != marker {
        return None;
    }
    if crate::cloud_environment::validate_lease(lease, session, host, now).is_err() {
        crate::logging::warn("Cloud resource access denied: lease validation failed");
        return None;
    }
    let updated = match chrono::DateTime::parse_from_rfc3339(&lease.updated_at) {
        Ok(updated) => updated.with_timezone(&chrono::Utc),
        Err(_) => {
            crate::logging::warn("Cloud resource access denied: invalid lease timestamp");
            return None;
        }
    };
    let age = match now.signed_duration_since(updated).to_std() {
        Ok(age) => age,
        Err(_) => {
            crate::logging::warn("Cloud resource access denied: lease timestamp is in the future");
            return None;
        }
    };
    Duration::from_secs(crate::cloud_environment::LOCAL_ACCESS_SECONDS as u64)
        .checked_sub(age)
        .filter(|left| !left.is_zero())
}
fn environment_marker(name: &str) -> Option<String> {
    match std::env::var(name) {
        Ok(value) => Some(value),
        Err(std::env::VarError::NotPresent) => None,
        Err(std::env::VarError::NotUnicode(_)) => {
            crate::logging::warn(&format!(
                "Cloud resource access denied: {name} is not Unicode"
            ));
            None
        }
    }
}

fn remaining(session: &str) -> Option<Duration> {
    lease_remaining(
        &crate::storage::read_session_lease(session)?,
        session,
        &environment_marker("JCODE_CLOUD_ENV_SESSION")?,
        &environment_marker("JCODE_SSH_REMOTE")?,
        chrono::Utc::now(),
    )
}
fn installation_ready(is_processing: bool, history_loaded: bool) -> bool {
    !is_processing && history_loaded
}
fn configuration() -> SessionToolConfig {
    SessionToolConfig {
        custom: vec![SessionToolDefinition {
            name: "cloud_environment".into(),
            description: "Temporary scoped local resource bridge. Before continuing a migrated task, call status then sync to restore the saved resource manifest. Fetch required resources opaquely into cloud files. Returns status only, never secret values or file content. Records a metadata manifest. No arbitrary shell commands. Access expires after 15 minutes without renewal.".into(),
            parameters: json!({"type":"object","additionalProperties":false,"required":["action"],"properties":{
                "action":{"type":"string","enum":["status","sync","fetch"]},
                "reason":{"type":"string","maxLength":1024},
                "resource":{"oneOf":[
                    {"type":"object","additionalProperties":false,"required":["kind","path"],"properties":{"kind":{"const":"project_file"},"path":{"type":"string"}}},
                    {"type":"object","additionalProperties":false,"required":["kind","name"],"properties":{"kind":{"const":"env_var"},"name":{"type":"string"}}},
                    {"type":"object","additionalProperties":false,"required":["kind","name"],"properties":{"kind":{"const":"credential"},"name":{"enum":["github","aws","npm"]}}}
                ]}
            }}),
        }], ..Default::default()
    }
}
fn valid_request(input: &Value) -> bool {
    match serde_json::to_vec(input) {
        Ok(bytes) => crate::cloud_environment::parse_request(&bytes).is_ok(),
        Err(_) => {
            crate::logging::warn("Cloud resource request could not be encoded");
            false
        }
    }
}
fn safe_status(raw: &[u8]) -> Option<String> {
    let status: crate::cloud_environment::Status = match serde_json::from_slice(raw) {
        Ok(status) => status,
        Err(_) => {
            crate::logging::warn("Cloud resource response rejected: invalid status JSON");
            return None;
        }
    };
    if !matches!(
        status.code.as_str(),
        "available" | "received" | "unavailable"
    ) || status.applied > 256
        || status.skipped > 256
    {
        return None;
    }
    // Only the typed, closed status object may cross into model context.
    match serde_json::to_string(&status) {
        Ok(status) => Some(status),
        Err(_) => {
            crate::logging::warn("Cloud resource response rejected: status encoding failed");
            None
        }
    }
}
async fn fulfill(session: &str, input: &Value) -> anyhow::Result<String> {
    let mut command = tokio::process::Command::new(
        std::env::current_exe().context("locate resource callback executable")?,
    );
    command
        .args([
            "--no-update",
            "--no-selfdev",
            "cloud",
            "env",
            "fulfill",
            "--session",
            session,
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.as_std_mut().process_group(0);
    }
    let mut child = command.spawn().context("spawn resource callback")?;
    #[cfg(unix)]
    let _group = FulfillProcessGroup(child.id().context("resource callback has no process ID")?);
    let mut stdin = child
        .stdin
        .take()
        .context("resource callback has no stdin")?;
    stdin
        .write_all(&serde_json::to_vec(input).context("encode resource request")?)
        .await
        .context("write resource callback input")?;
    stdin
        .shutdown()
        .await
        .context("close resource callback input")?;
    drop(stdin);
    let mut output = Vec::new();
    child
        .stdout
        .take()
        .context("resource callback has no stdout")?
        .take(16385)
        .read_to_end(&mut output)
        .await
        .context("read resource callback output")?;
    if output.len() > 16384 {
        anyhow::bail!("resource callback output exceeds limit");
    }
    if !child
        .wait()
        .await
        .context("wait for resource callback")?
        .success()
    {
        anyhow::bail!("resource callback exited unsuccessfully");
    }
    safe_status(&output).context("resource callback returned invalid status")
}

#[cfg(unix)]
struct FulfillProcessGroup(u32);
#[cfg(unix)]
impl Drop for FulfillProcessGroup {
    fn drop(&mut self) {
        // The CLI and its SSH transport are isolated in this group. Cancel all
        // descendants too, including when this future is aborted on a switch.
        unsafe {
            libc::kill(-(self.0 as i32), libc::SIGKILL);
        }
    }
}

impl App {
    pub(super) fn cloud_environment_idle_clear_due(&self) -> bool {
        !self.is_processing
            && self.cloud_environment.grant.as_ref().is_some_and(|grant| {
                grant.clear_after_idle
                    && (!grant.live.load(Ordering::SeqCst)
                        || tokio::time::Instant::now() >= grant.deadline)
            })
    }
    pub(super) fn revoke_cloud_environment(&mut self) {
        self.cloud_environment.grant = None;
    }
    pub(super) fn cloud_environment_session_changed(&mut self, next: &str) {
        if self
            .remote_session_id
            .as_deref()
            .is_some_and(|old| old != next)
        {
            self.revoke_cloud_environment();
            self.cloud_environment.switched_away = true;
        }
    }
    pub(super) async fn prepare_cloud_environment(
        &mut self,
        remote: &mut RemoteConnection,
    ) -> anyhow::Result<()> {
        let Some(session) = self.remote_session_id.clone() else {
            return Ok(());
        };
        if self.cloud_environment.switched_away {
            return Ok(());
        }
        if self
            .cloud_environment
            .grant
            .as_ref()
            .is_some_and(|g| g.session != session)
        {
            self.revoke_cloud_environment();
        }
        if let Some(grant) = &mut self.cloud_environment.grant {
            if remaining(&session).is_none() || tokio::time::Instant::now() >= grant.deadline {
                grant.live.store(false, Ordering::SeqCst);
                for task in grant.tasks.drain(..) {
                    task.abort();
                }
                if grant.clear_after_idle && !self.is_processing {
                    remote.configure_cloud_tools(Default::default()).await?;
                    grant.clear_after_idle = false;
                }
            }
            // Never renew even if another operation rewrites the lease timestamp.
            return Ok(());
        }
        // On reconnect, SessionId can arrive before History tells us that a
        // detached turn is still active. ConfigureTools is rejected mid-turn.
        if !installation_ready(self.is_processing, remote.has_loaded_history()) {
            return Ok(());
        }
        let Some(left) = remaining(&session) else {
            return Ok(());
        };
        let deadline = *self
            .cloud_environment
            .deadline
            .get_or_insert_with(|| tokio::time::Instant::now() + left);
        if tokio::time::Instant::now() >= deadline {
            return Ok(());
        }
        remote.configure_cloud_tools(configuration()).await?;
        let live = Arc::new(AtomicBool::new(true));
        remote.bind_cloud_authority(&live);
        let expiry = RemoteConnection::cloud_expiry_revoke(live.clone(), deadline);
        self.cloud_environment.grant = Some(Grant {
            session,
            deadline,
            live,
            tasks: vec![expiry],
            clear_after_idle: true,
        });
        Ok(())
    }
    pub(super) fn cloud_environment_call(
        &mut self,
        remote: &mut RemoteConnection,
        session: String,
        call_id: String,
        name: String,
        input: Value,
    ) {
        let reply = remote.cloud_callback_sender();
        let authorized =
            name == "cloud_environment" && valid_request(&input) && remaining(&session).is_some();
        let expected = self.remote_session_id.as_deref();
        let Some(grant) = self
            .cloud_environment
            .grant
            .as_mut()
            .filter(|g| authorized && g.allows(expected, &session))
        else {
            reply(
                call_id,
                String::new(),
                Some("Local resource access denied".into()),
            );
            return;
        };
        grant.tasks.retain(|task| !task.is_finished());
        if grant.tasks.len() >= 9 {
            reply(
                call_id,
                String::new(),
                Some("Local resource bridge is busy".into()),
            );
            return;
        }
        let live = grant.live.clone();
        let deadline = grant.deadline;
        let task = tokio::spawn(async move {
            let limit = deadline.min(tokio::time::Instant::now() + Duration::from_secs(75));
            let result = match tokio::time::timeout_at(limit, fulfill(&session, &input)).await {
                Ok(Ok(output)) => Some(output),
                Ok(Err(_)) => {
                    // Never log resource bytes or subprocess diagnostics.
                    crate::logging::warn("Cloud resource callback failed");
                    None
                }
                Err(_) => {
                    crate::logging::warn("Cloud resource callback timed out");
                    None
                }
            };
            if live.load(Ordering::SeqCst)
                && remaining(&session).is_some()
                && tokio::time::Instant::now() < deadline
                && let Some(output) = result
            {
                reply(call_id, output, None);
                return;
            }
            reply(
                call_id,
                String::new(),
                Some("Local resource request failed or expired".into()),
            );
        });
        remote.track_cloud_callback(&task);
        grant.tasks.push(task);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn installation_waits_for_idle_loaded_history() {
        assert!(!installation_ready(true, false));
        assert!(!installation_ready(true, true));
        assert!(!installation_ready(false, false));
        assert!(installation_ready(false, true));
    }

    #[test]
    fn revoked_expiry_retries_clear_once_when_idle() {
        use tokio::io::AsyncBufReadExt;
        let mut app = super::super::tests::create_test_app();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let mut remote = RemoteConnection::dummy();
            let peer = remote.take_dummy_peer().unwrap();
            let (reader, _writer) = peer.into_split();
            let mut reader = tokio::io::BufReader::new(reader);
            app.remote_session_id = Some("expired-bridge-test".into());
            app.is_processing = true;
            app.cloud_environment.grant = Some(Grant {
                session: "expired-bridge-test".into(),
                deadline: tokio::time::Instant::now(),
                // The timer already revoked access during an active turn.
                // Its false live flag must not suppress later idle cleanup.
                live: Arc::new(AtomicBool::new(false)),
                tasks: vec![],
                clear_after_idle: true,
            });
            assert!(!app.cloud_environment_idle_clear_due());
            app.prepare_cloud_environment(&mut remote).await.unwrap();
            let mut line = String::new();
            assert!(
                tokio::time::timeout(Duration::from_millis(10), reader.read_line(&mut line))
                    .await
                    .is_err()
            );
            assert!(
                app.cloud_environment
                    .grant
                    .as_ref()
                    .unwrap()
                    .clear_after_idle
            );

            app.is_processing = false;
            assert!(app.cloud_environment_idle_clear_due());
            app.prepare_cloud_environment(&mut remote).await.unwrap();
            reader.read_line(&mut line).await.unwrap();
            let request: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(request["type"], "configure_tools");
            assert_eq!(request["tools"]["custom"], json!([]));
            assert!(!app.cloud_environment_idle_clear_due());
            assert!(
                !app.cloud_environment
                    .grant
                    .as_ref()
                    .unwrap()
                    .clear_after_idle
            );
            for _ in 0..3 {
                app.prepare_cloud_environment(&mut remote).await.unwrap();
            }
            line.clear();
            assert!(
                tokio::time::timeout(Duration::from_millis(10), reader.read_line(&mut line))
                    .await
                    .is_err()
            );
        });
    }
    #[test]
    fn callback_grant_is_bound_to_active_session_and_expiry() {
        let live = Arc::new(AtomicBool::new(true));
        let mut grant = Grant {
            session: "expected".into(),
            deadline: tokio::time::Instant::now() + Duration::from_secs(30),
            live: live.clone(),
            tasks: vec![],
            clear_after_idle: true,
        };
        assert!(grant.allows(Some("expected"), "expected"));
        assert!(!grant.allows(Some("expected"), "other"));
        assert!(!grant.allows(Some("other"), "expected"));
        assert!(!grant.allows(None, "expected"));
        grant.deadline = tokio::time::Instant::now();
        assert!(!grant.allows(Some("expected"), "expected"));
        grant.deadline += Duration::from_secs(30);
        live.store(false, Ordering::SeqCst);
        assert!(!grant.allows(Some("expected"), "expected"));
    }
    #[test]
    fn dropping_grant_revokes_shared_authority() {
        let live = Arc::new(AtomicBool::new(true));
        drop(Grant {
            session: "session".into(),
            deadline: tokio::time::Instant::now() + Duration::from_secs(30),
            live: live.clone(),
            tasks: vec![],
            clear_after_idle: true,
        });
        assert!(!live.load(Ordering::SeqCst));
    }
    #[test]
    fn callback_request_is_closed_and_path_scoped() {
        assert!(valid_request(
            &json!({"action":"fetch","resource":{"kind":"env_var","name":"API_KEY"}})
        ));
        for request in [
            json!({"action":"shell"}),
            json!({"action":"fetch"}),
            json!({"action":"status","value":"secret"}),
            json!({"action":"fetch","resource":{"kind":"project_file","path":"../secret"}}),
            json!({"action":"fetch","resource":{"kind":"credential","name":"ssh"}}),
        ] {
            assert!(
                !valid_request(&request),
                "unexpected accepted request: {request}"
            );
        }
    }
    #[test]
    fn outputs_never_forward_secret_fields() {
        assert!(
            safe_status(br#"{"ok":true,"code":"available","applied":0,"skipped":1}"#).is_some()
        );
        assert!(
            safe_status(
                br#"{"ok":true,"code":"available","applied":0,"skipped":1,"value":"secret"}"#
            )
            .is_none()
        );
        assert!(safe_status(br#"{"ok":true,"code":"secret","applied":0,"skipped":0}"#).is_none());
        assert!(
            safe_status(br#"{"ok":true,"code":"received","applied":257,"skipped":0}"#).is_none()
        );
        assert!(safe_status(b"not json").is_none());
    }
    #[test]
    fn grant_requires_exact_session_host_root_and_fresh_time() {
        let now = chrono::Utc::now();
        let root = tempfile::tempdir().unwrap();
        let mut lease = crate::storage::SessionLease {
            session_id: "session".into(),
            epoch: 1,
            away_host: Some("host".into()),
            repo_root: Some(root.path().canonicalize().unwrap().to_str().unwrap().into()),
            last_sync_snapshot: None,
            last_sync_head: None,
            updated_at: now.to_rfc3339(),
        };
        assert!(lease_remaining(&lease, "session", "session", "host", now).is_some());
        assert!(lease_remaining(&lease, "other", "session", "host", now).is_none());
        assert!(lease_remaining(&lease, "session", "session", "other", now).is_none());
        lease.updated_at = (now - chrono::Duration::seconds(900)).to_rfc3339();
        assert!(lease_remaining(&lease, "session", "session", "host", now).is_none());
        lease.updated_at = (now + chrono::Duration::seconds(1)).to_rfc3339();
        assert!(lease_remaining(&lease, "session", "session", "host", now).is_none());
    }
}
