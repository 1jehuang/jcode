//! Real OS-process regression tests for the public role update API.
use super::Config;
use std::cell::RefCell;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command};
use std::time::Duration;

thread_local! {
    static BARRIER: RefCell<Option<(TcpStream, bool, bool)>> = const { RefCell::new(None) };
}

pub(super) fn loaded() {
    BARRIER.with(|barrier| {
        if let Some((stream, hold, _)) = barrier.borrow_mut().as_mut()
            && *hold
        {
            stream.write_all(b"LOADED\n").unwrap();
            let mut release = String::new();
            BufReader::new(stream).read_line(&mut release).unwrap();
            assert_eq!(release, "RELEASE\n");
        }
    });
}

pub(super) fn contended() {
    BARRIER.with(|barrier| {
        if let Some((stream, hold, notified)) = barrier.borrow_mut().as_mut()
            && !*hold
            && !*notified
        {
            stream.write_all(b"BLOCKED\n").unwrap();
            *notified = true;
        }
    });
}

#[test]
fn role_update_process_worker() {
    let Ok(address) = std::env::var("JCODE_ROLE_TEST_BARRIER") else {
        return;
    };
    let role = std::env::var("JCODE_ROLE_TEST_TARGET").unwrap();
    let stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    BARRIER.with(|barrier| *barrier.borrow_mut() = Some((stream, role == "review", false)));
    Config::set_agent_model_default(&role, Some(&format!("server-{role}"))).unwrap();
}

struct Worker(Child);
impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn worker(home: &std::path::Path, target: &str) -> (Worker, BufReader<TcpStream>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "config::config_file::role_transaction_tests::role_update_process_worker",
            "--nocapture",
        ])
        .env("JCODE_HOME", home)
        .env("JCODE_ROLE_TEST_BARRIER", address.to_string())
        .env("JCODE_ROLE_TEST_TARGET", target)
        .spawn()
        .unwrap();
    let child = Worker(child);
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(listener.accept());
    });
    let (stream, _) = rx
        .recv_timeout(Duration::from_secs(10))
        .expect("worker must connect")
        .unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    (child, BufReader::new(stream))
}

#[test]
fn distinct_process_role_updates_preserve_both_roles() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(
        home.path().join("config.toml"),
        "[display]\ncentered = false\n",
    )
    .unwrap();
    let (mut first, mut first_barrier) = worker(home.path(), "review");
    let mut signal = String::new();
    first_barrier.read_line(&mut signal).unwrap();
    assert_eq!(
        signal, "LOADED\n",
        "first process holds the lock after reading"
    );
    let (mut second, mut second_barrier) = worker(home.path(), "judge");
    signal.clear();
    second_barrier.read_line(&mut signal).unwrap();
    assert_eq!(
        signal, "BLOCKED\n",
        "second process must contest the transaction lock"
    );
    // Release only after observing real OS lock contention, not elapsed time.
    first_barrier.get_mut().write_all(b"RELEASE\n").unwrap();
    assert!(first.0.wait().unwrap().success());
    assert!(second.0.wait().unwrap().success());
    let saved: Config =
        toml::from_str(&std::fs::read_to_string(home.path().join("config.toml")).unwrap()).unwrap();
    assert_eq!(
        (
            saved.autoreview.model.as_deref(),
            saved.autojudge.model.as_deref()
        ),
        (Some("server-review"), Some("server-judge"))
    );
    assert!(!saved.display.centered);
    assert!(home.path().join("config.toml.lock").exists());
}

struct TestHome {
    directory: tempfile::TempDir,
    previous: Option<std::ffi::OsString>,
}
impl TestHome {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let previous = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", directory.path());
        Self {
            directory,
            previous,
        }
    }
    fn path(&self) -> std::path::PathBuf {
        self.directory.path().join("config.toml")
    }
}
impl Drop for TestHome {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => crate::env::set_var("JCODE_HOME", value),
            None => crate::env::remove_var("JCODE_HOME"),
        }
        Config::invalidate_cache();
    }
}

#[test]
fn role_update_refuses_malformed_config_without_replacement() {
    let _guard = crate::storage::lock_test_env();
    let home = TestHome::new();
    let original = "[autoreview\nmodel = 'keep-me'\n";
    std::fs::write(home.path(), original).unwrap();
    assert!(Config::set_agent_model_default("review", Some("new")).is_err());
    assert_eq!(std::fs::read_to_string(home.path()).unwrap(), original);
}

#[test]
fn role_update_rejects_unknown_target_without_replacement() {
    let _guard = crate::storage::lock_test_env();
    let home = TestHome::new();
    let original = "[autoreview]\nmodel = 'keep-me'\n";
    std::fs::write(home.path(), original).unwrap();
    assert!(Config::set_agent_model_default("unknown", Some("new")).is_err());
    assert_eq!(std::fs::read_to_string(home.path()).unwrap(), original);
}

#[test]
fn role_update_trim_clear_and_inherit_preserve_distinct_roles() {
    let _guard = crate::storage::lock_test_env();
    let _home = TestHome::new();
    for role in ["swarm", "review", "judge", "memory", "ambient"] {
        Config::set_agent_model_default(role, Some("  inherit  ")).unwrap();
    }
    Config::set_agent_model_default("review", Some("   ")).unwrap();
    let saved = Config::load_strict().unwrap();
    assert_eq!(saved.autoreview.model, None);
    assert_eq!(
        (
            saved.agents.swarm_model.as_deref(),
            saved.autojudge.model.as_deref(),
            saved.agents.memory_model.as_deref(),
            saved.ambient.model.as_deref()
        ),
        (
            Some("inherit"),
            Some("inherit"),
            Some("inherit"),
            Some("inherit")
        )
    );
}

#[cfg(unix)]
#[test]
fn role_update_atomic_replacement_preserves_existing_permissions() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let _guard = crate::storage::lock_test_env();
    let home = TestHome::new();
    std::fs::write(home.path(), "[autoreview]\nmodel = 'old'\n").unwrap();
    std::fs::set_permissions(home.path(), std::fs::Permissions::from_mode(0o640)).unwrap();
    let old_file = std::fs::File::open(home.path()).unwrap();
    Config::set_agent_model_default("review", Some("new")).unwrap();
    let current = std::fs::metadata(home.path()).unwrap();
    assert_eq!(current.permissions().mode() & 0o777, 0o640);
    assert_ne!(current.ino(), old_file.metadata().unwrap().ino());
}
