use super::*;
use std::os::unix::fs::PermissionsExt;

struct TestHome(Option<std::ffi::OsString>, tempfile::TempDir);
impl TestHome {
    fn new() -> Self {
        let old = std::env::var_os("JCODE_HOME");
        let dir = tempfile::tempdir().unwrap();
        crate::env::set_var("JCODE_HOME", dir.path());
        Self(old, dir)
    }
}
impl Drop for TestHome {
    fn drop(&mut self) {
        match self.0.take() {
            Some(old) => crate::env::set_var("JCODE_HOME", old),
            None => crate::env::remove_var("JCODE_HOME"),
        }
    }
}

struct IdleProvider;
#[async_trait::async_trait]
impl crate::provider::Provider for IdleProvider {
    async fn complete(
        &self,
        _: &[crate::message::Message],
        _: &[crate::message::ToolDefinition],
        _: &str,
        _: Option<&str>,
    ) -> Result<crate::provider::EventStream> {
        bail!("checkpoint must never invoke a provider")
    }
    fn name(&self) -> &str {
        "mock"
    }
    fn fork(&self) -> Arc<dyn crate::provider::Provider> {
        Arc::new(Self)
    }
}

fn idle_agent() -> crate::agent::Agent {
    crate::agent::Agent::new(Arc::new(IdleProvider), crate::tool::Registry::empty())
}

#[test]
fn checkpoint_preserves_blank_session_ids_overrides_and_queued_input() {
    let _env = crate::storage::lock_test_env();
    let _home = TestHome::new();
    let mut blank = idle_agent();
    let blank_id = blank.session_id().to_string();
    blank.checkpoint_idle_runtime_upgrade().unwrap();
    assert_eq!(
        crate::session::Session::load(&blank_id).unwrap().id,
        blank_id
    );

    let mut configured = idle_agent();
    configured.set_system_prompt("Keep this custom prompt after runtime reconnect");
    configured
        .set_subagent_model(Some("test-model".into()))
        .unwrap();
    let queue = configured.soft_interrupt_queue();
    queue
        .lock()
        .unwrap()
        .push(jcode_agent_runtime::SoftInterruptMessage {
            content: "accepted follow-up".into(),
            images: Vec::new(),
            urgent: false,
            source: jcode_agent_runtime::SoftInterruptSource::User,
            client_id: Some("follow-up-7".into()),
        });
    configured.checkpoint_idle_runtime_upgrade().unwrap();
    let restored = crate::session::Session::load(configured.session_id()).unwrap();
    assert_eq!(
        restored.system_prompt.as_deref(),
        Some("Keep this custom prompt after runtime reconnect")
    );
    assert_eq!(restored.subagent_model.as_deref(), Some("test-model"));
    let pending = crate::soft_interrupt_store::load(configured.session_id()).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].client_id.as_deref(), Some("follow-up-7"));
}

#[test]
fn checkpoint_refuses_queue_write_failure_instead_of_losing_accepted_input() {
    let _env = crate::storage::lock_test_env();
    let home = TestHome::new();
    let mut agent = idle_agent();
    agent
        .soft_interrupt_queue()
        .lock()
        .unwrap()
        .push(jcode_agent_runtime::SoftInterruptMessage {
            content: "never lose this input".into(),
            images: Vec::new(),
            urgent: false,
            source: jcode_agent_runtime::SoftInterruptSource::User,
            client_id: None,
        });
    std::fs::write(
        home.1.path().join("pending-soft-interrupts"),
        "not a directory",
    )
    .unwrap();
    assert!(agent.checkpoint_idle_runtime_upgrade().is_err());
    assert_eq!(agent.soft_interrupt_queue().lock().unwrap().len(), 1);
}

#[test]
fn busy_agent_checkpoint_never_waits_and_idle_index_stays_reserved() {
    let _env = crate::storage::lock_test_env();
    // Serialize process-wide environment outside the asynchronous runtime.
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime")
        .block_on(async {
            let _home = TestHome::new();
            let agent = Arc::new(Mutex::new(idle_agent()));
            let sessions = Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::from([
                ("idle".into(), agent.clone()),
            ])));
            let busy = agent.lock().await;
            assert!(checkpoint_sessions(&sessions).is_err());
            drop(busy);
            let checkpoint = checkpoint_sessions(&sessions).unwrap();
            assert!(agent.try_lock().is_err());
            assert!(sessions.try_write().is_err());
            drop(checkpoint);
            assert!(agent.try_lock().is_ok());
        });
}

fn candidate() -> (tempfile::TempDir, PathBuf, FileIdentity) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("jcode");
    std::fs::write(&path, b"\x7fELFfixture").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    let (path, metadata) = validate_path(&path).unwrap();
    (dir, path, metadata)
}

#[test]
fn idle_upgrade_reserves_explicit_candidate_and_blocks_racing_turn() {
    let (_dir, path, metadata) = candidate();
    let gate = ActivityGate::default();
    let UpgradeDecision::Ready(prepared) =
        reserve_upgrade(path.clone(), metadata, "1.2.4", "1.2.3", &gate).unwrap()
    else {
        panic!("expected idle upgrade")
    };
    assert_eq!(prepared.path, path);
    assert!(gate.try_enter().is_err());
    assert!(gate.try_idle().is_none());
    drop(prepared); // An exec failure releases the reservation, not the daemon.
    assert!(gate.try_enter().is_ok());
}

#[test]
fn changed_candidate_after_progress_refuses_exec_and_releases_idle_gate() {
    let (_dir, path, metadata) = candidate();
    let gate = ActivityGate::default();
    let UpgradeDecision::Ready(prepared) =
        reserve_upgrade(path.clone(), metadata, "1.2.4", "1.2.3", &gate).unwrap()
    else {
        panic!("expected idle upgrade")
    };
    std::fs::write(&path, b"\x7fELFchanged-after-progress").unwrap();
    let mut command = std::process::Command::new("/must-not-exec");
    let error = replace_after_delivery_checkpoint(&mut command, 0, &prepared).unwrap_err();
    assert!(error.to_string().contains("candidate changed"));
    drop(prepared);
    assert!(gate.try_enter().is_ok());
}

#[test]
fn failed_exec_releases_publication_fence_without_reentrant_logging() {
    let (_dir, path, metadata) = candidate();
    let gate = ActivityGate::default();
    let UpgradeDecision::Ready(prepared) =
        reserve_upgrade(path, metadata, "1.2.4", "1.2.3", &gate).unwrap()
    else {
        panic!("expected idle upgrade")
    };
    let bus = crate::bus::Bus::global();
    let checkpoint = bus.runtime_delivery_checkpoint().unwrap();
    // Never execute a fixture or a real runtime from this unit test. A missing
    // command deterministically exercises exec's error return in this process.
    let mut command = std::process::Command::new("/jcode-test-missing-runtime/no-exec");
    let error = replace_after_delivery_checkpoint(&mut command, checkpoint, &prepared).unwrap();
    assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
    assert!(bus.try_runtime_delivery_idle(checkpoint).is_some());
    drop(prepared);
    assert!(gate.try_enter().is_ok());
}

#[test]
fn active_turn_or_tool_refuses_upgrade_without_firing_cancel() {
    let (_dir, path, metadata) = candidate();
    let gate = ActivityGate::default();
    let signal = jcode_agent_runtime::InterruptSignal::new();
    let turn = gate.try_enter().unwrap();
    let tool = gate.try_enter().unwrap();
    assert!(matches!(
        reserve_upgrade(path.clone(), metadata.clone(), "2.0.0", "1.0.0", &gate).unwrap(),
        UpgradeDecision::Busy
    ));
    assert!(!signal.is_set());
    drop(turn);
    assert!(matches!(
        reserve_upgrade(path, metadata, "2.0.0", "1.0.0", &gate).unwrap(),
        UpgradeDecision::Busy
    ));
    assert!(!signal.is_set());
    drop(tool);
    assert!(gate.try_idle().is_some());
}

#[test]
fn no_downgrade_or_same_version_reload_even_when_candidate_has_new_mtime() {
    let (_dir, path, metadata) = candidate();
    let gate = ActivityGate::default();
    for version in ["0.99.99", "1.2.3"] {
        assert!(matches!(
            reserve_upgrade(path.clone(), metadata.clone(), version, "1.2.3", &gate).unwrap(),
            UpgradeDecision::Unchanged
        ));
        assert!(gate.try_enter().is_ok());
    }
    for version in ["garbage", "1.2", "1.2.3.4", "1.2.3-dev"] {
        assert!(reserve_upgrade(path.clone(), metadata.clone(), version, "1.2.3", &gate).is_err());
    }
}

#[test]
fn rejects_invalid_candidate_paths_scripts_permissions_and_changes() {
    let (dir, path, metadata) = candidate();
    assert!(validate_path(Path::new("jcode")).is_err());
    assert!(validate_path(dir.path()).is_err());
    assert!(validate_path(&dir.path().join("missing")).is_err());
    std::fs::write(&path, b"#!/bin/sh\nexit 0\n").unwrap();
    assert!(validate_path(&path).is_err());
    assert!(
        reserve_upgrade(
            path.clone(),
            metadata,
            "2.0.0",
            "1.0.0",
            &ActivityGate::default()
        )
        .is_err()
    );
    std::fs::write(&path, b"\x7fELFfixture").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o777)).unwrap();
    assert!(validate_path(&path).is_err());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(validate_path(&path).is_err());
}

#[tokio::test]
async fn version_probe_rejects_non_jcode_executable() {
    assert!(probe_version(Path::new("/bin/true")).await.is_err());
}

#[tokio::test]
async fn invalid_native_upgrade_returns_status_without_exec_or_agent() {
    use tokio::io::AsyncBufReadExt;
    let (client, server) = crate::transport::stream_pair().unwrap();
    let (_, write) = server.into_split();
    let sessions = Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new()));
    handle(7, "relative-jcode", &Arc::new(Mutex::new(write)), &sessions)
        .await
        .unwrap();
    let mut line = String::new();
    tokio::io::BufReader::new(client)
        .read_line(&mut line)
        .await
        .unwrap();
    let reply: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(reply["type"], "reload_progress");
    assert_eq!(reply["step"], "invalid");
    assert_eq!(reply["success"], false);
}

#[test]
fn upgrade_wire_request_is_lightweight_and_never_legacy_reload() {
    let request = crate::protocol::decode_request(
        r#"{"type":"upgrade_runtime","id":9,"candidate":"/bundle/jcode"}"#,
    )
    .unwrap();
    assert!(request.is_lightweight_control_request());
    assert_eq!(request.id(), 9);
    assert!(matches!(
        request,
        crate::protocol::Request::UpgradeRuntime { .. }
    ));
}
