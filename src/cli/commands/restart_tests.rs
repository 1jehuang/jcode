use super::{
    maybe_run_pending_restart_restore_on_startup, run_restart_clear_command,
    run_restart_restore_command, run_restart_save_command,
};
use crate::session::Session;
use std::ffi::OsString;

struct TestEnvGuard {
    prev_home: Option<OsString>,
    prev_spawn_hook: Option<OsString>,
    _temp_home: tempfile::TempDir,
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl TestEnvGuard {
    fn new() -> anyhow::Result<Self> {
        let lock = crate::storage::lock_test_env();
        let temp_home = tempfile::Builder::new()
            .prefix("jcode-cli-restart-test-home-")
            .tempdir()?;
        let prev_home = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", temp_home.path());
        Ok(Self {
            prev_home,
            prev_spawn_hook: std::env::var_os("JCODE_SPAWN_HOOK"),
            _temp_home: temp_home,
            _lock: lock,
        })
    }
}

impl Drop for TestEnvGuard {
    fn drop(&mut self) {
        if let Some(hook) = &self.prev_spawn_hook {
            crate::env::set_var("JCODE_SPAWN_HOOK", hook);
        } else {
            crate::env::remove_var("JCODE_SPAWN_HOOK");
        }
        if let Some(prev_home) = &self.prev_home {
            crate::env::set_var("JCODE_HOME", prev_home);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }
}

#[tokio::test]
async fn restart_save_writes_empty_snapshot_with_auto_restore_flag() {
    let _guard = TestEnvGuard::new().expect("setup test env");

    run_restart_save_command(true)
        .await
        .expect("save restart snapshot");

    let snapshot = crate::restart_snapshot::load_snapshot().expect("load snapshot");
    assert!(snapshot.auto_restore_on_next_start);
    assert!(snapshot.sessions.is_empty());
}

#[tokio::test]
async fn pending_restore_returns_false_for_unarmed_snapshot() {
    let _guard = TestEnvGuard::new().expect("setup test env");

    run_restart_save_command(false)
        .await
        .expect("save restart snapshot");

    assert!(
        !maybe_run_pending_restart_restore_on_startup()
            .await
            .expect("check pending restore")
    );
    assert!(crate::restart_snapshot::load_snapshot().is_ok());
}

#[tokio::test]
async fn pending_restore_does_not_auto_restore_recent_crash_without_snapshot() {
    let _guard = TestEnvGuard::new().expect("setup test env");

    let mut child = std::process::Command::new("sh")
        .arg("-c")
        .arg("exit 0")
        .spawn()
        .expect("spawn child");
    let dead_pid = child.id();
    let _ = child.wait().expect("wait for child");

    let mut crashed = Session::create_with_id(
        "session_no_startup_auto_restore_crash".to_string(),
        None,
        Some("Do Not Respawn".to_string()),
    );
    crashed.mark_active_with_pid(dead_pid);
    crashed.save().expect("save active session with dead pid");

    assert!(
        !maybe_run_pending_restart_restore_on_startup()
            .await
            .expect("check pending restore")
    );
    assert!(crate::restart_snapshot::load_snapshot().is_err());
}

#[tokio::test]
async fn restart_clear_removes_saved_snapshot() {
    let _guard = TestEnvGuard::new().expect("setup test env");

    run_restart_save_command(false)
        .await
        .expect("save restart snapshot");
    run_restart_clear_command().expect("clear restart snapshot");

    assert!(crate::restart_snapshot::load_snapshot().is_err());
}

#[test]
fn stale_snapshot_requires_noninteractive_confirmation_without_launching() {
    let _guard = TestEnvGuard::new().expect("setup test env");
    let snapshot = crate::restart_snapshot::RestartSnapshot {
        version: 1,
        created_at: chrono::Utc::now() - chrono::Duration::hours(47),
        auto_restore_on_next_start: false,
        sessions: vec![crate::restart_snapshot::RestartSnapshotSession {
            session_id: "stale-session".to_string(),
            display_name: "Stale session".to_string(),
            working_dir: None,
            is_selfdev: false,
        }],
    };
    crate::restart_snapshot::write_snapshot(&snapshot).expect("write snapshot");
    let error = run_restart_restore_command(false).expect_err("stale restore must require consent");
    assert!(error.to_string().contains("--yes"));
    assert!(!crate::restart_snapshot::is_passive_restore("stale-session").expect("marker"));
    assert!(crate::restart_snapshot::load_snapshot().is_ok());
}

#[tokio::test]
async fn multi_session_auto_restore_disarms_without_launching() {
    let _guard = TestEnvGuard::new().expect("setup test env");
    let snapshot = crate::restart_snapshot::RestartSnapshot {
        version: 1,
        created_at: chrono::Utc::now(),
        auto_restore_on_next_start: true,
        sessions: ["one", "two"]
            .into_iter()
            .map(|id| crate::restart_snapshot::RestartSnapshotSession {
                session_id: id.to_string(),
                display_name: id.to_string(),
                working_dir: None,
                is_selfdev: false,
            })
            .collect(),
    };
    crate::restart_snapshot::write_snapshot(&snapshot).expect("write snapshot");
    assert!(
        !maybe_run_pending_restart_restore_on_startup()
            .await
            .expect("check restore")
    );
    assert!(
        !crate::restart_snapshot::load_snapshot()
            .expect("snapshot kept")
            .auto_restore_on_next_start
    );
    assert!(!crate::restart_snapshot::is_passive_restore("one").expect("marker"));
}

#[cfg(unix)]
#[tokio::test]
async fn manual_restore_disarms_retained_snapshot() -> anyhow::Result<()> {
    let _guard = TestEnvGuard::new()?;
    use std::os::unix::fs::PermissionsExt;
    let home = crate::storage::jcode_dir()?;
    let hook = home.join("capture-spawn");
    std::fs::write(
        &hook,
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$JCODE_HOME/launches\"\n",
    )?;
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o700))?;
    crate::env::set_var("JCODE_SPAWN_HOOK", &hook);
    crate::restart_snapshot::write_snapshot(&crate::restart_snapshot::RestartSnapshot {
        version: 1,
        created_at: chrono::Utc::now(),
        auto_restore_on_next_start: true,
        sessions: vec![crate::restart_snapshot::RestartSnapshotSession {
            session_id: "manual-restore".into(),
            display_name: "manual restore".into(),
            working_dir: None,
            is_selfdev: false,
        }],
    })?;
    run_restart_restore_command(true)?;
    assert!(!crate::restart_snapshot::load_snapshot()?.auto_restore_on_next_start);
    assert!(!maybe_run_pending_restart_restore_on_startup().await?);
    let launches = std::fs::read_to_string(home.join("launches"))?;
    assert_eq!(launches.lines().count(), 1);
    assert!(launches.contains("--passive-restore"));
    Ok(())
}
