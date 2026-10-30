//! Tests for the cross-process mcp.json toggle lock.

use super::*;
use crate::mcp::protocol::McpConfig;
use std::collections::HashMap;

fn server_cfg(name: &str, enabled: bool) -> (String, crate::mcp::protocol::McpServerConfig) {
    (
        name.to_string(),
        crate::mcp::protocol::McpServerConfig {
            command: format!("{name}-binary"),
            args: vec![],
            env: HashMap::new(),
            shared: true,
            transport: None,
            url: None,
            headers: HashMap::new(),
            enabled: Some(enabled),
            disabled: Some(!enabled),
            timeout_secs: None,
        },
    )
}

fn config_with(servers: Vec<(String, crate::mcp::protocol::McpServerConfig)>) -> McpConfig {
    McpConfig {
        servers: servers.into_iter().collect(),
        ..McpConfig::default()
    }
}

struct JcodeHomeGuard {
    prev: Option<std::ffi::OsString>,
    _temp: tempfile::TempDir,
}

impl JcodeHomeGuard {
    fn new() -> Self {
        let prev = std::env::var_os("JCODE_HOME");
        let temp = tempfile::tempdir().expect("temp jcode home");
        crate::env::set_var("JCODE_HOME", temp.path());
        Self { prev, _temp: temp }
    }
}

impl Drop for JcodeHomeGuard {
    fn drop(&mut self) {
        if let Some(prev) = self.prev.take() {
            crate::env::set_var("JCODE_HOME", prev);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }
}

/// Two threads toggling different servers from the same stale snapshot must
/// both land in mcp.json. Without the lock, whichever save finished second
/// wrote its whole snapshot and erased the other's flag flip.
///
/// `lock_test_env` only serializes this test against other env-mutating tests;
/// the two threads below intentionally race with no shared mutex.
#[cfg(unix)]
#[test]
fn concurrent_toggles_of_different_servers_both_persist() {
    let _guard = crate::storage::lock_test_env();
    let home = JcodeHomeGuard::new();
    let snapshot = config_with(vec![server_cfg("alpha", true), server_cfg("beta", true)]);
    let path = crate::storage::jcode_dir()
        .expect("jcode dir")
        .join("mcp.json");
    snapshot
        .save_to_file(&path)
        .expect("seed mcp.json before toggles");

    let threads: Vec<_> = ["alpha", "beta"]
        .into_iter()
        .map(|server| {
            let snapshot = snapshot.clone();
            std::thread::spawn(move || {
                let (_, persistence) =
                    persist_toggle(&snapshot, server, false).expect("toggle persists");
                assert!(
                    persistence.save_ok,
                    "toggle of {server} must report a successful save: {:?}",
                    persistence.save_error
                );
            })
        })
        .collect();
    for thread in threads {
        thread.join().expect("toggle thread must not panic");
    }

    let saved = McpConfig::load_from_file(&path).expect("read back mcp.json after both toggles");
    assert_eq!(saved.servers["alpha"].enabled, Some(false));
    assert_eq!(saved.servers["beta"].enabled, Some(false));
    drop(home);
}

/// A second writer must block on the flock until the first releases it, then
/// re-read the file inside the lock and see the first writer's flip plus its
/// own, never its stale input snapshot.
#[cfg(unix)]
#[test]
fn second_writer_blocks_until_first_releases() {
    let _guard = crate::storage::lock_test_env();
    let home = JcodeHomeGuard::new();
    let jcode_dir = crate::storage::jcode_dir().expect("jcode dir");
    let path = jcode_dir.join("mcp.json");
    let stale = config_with(vec![server_cfg("alpha", true), server_cfg("beta", true)]);
    stale.save_to_file(&path).expect("seed mcp.json");

    let first = McpConfigLock::acquire(&jcode_dir).expect("first writer takes the lock");
    let before = std::time::Instant::now();
    let second_path = path.clone();
    let second = std::thread::spawn(move || {
        // Snapshot as if loaded before the first writer ran.
        let stale = config_with(vec![server_cfg("alpha", true), server_cfg("beta", true)]);
        let (_, persistence) =
            persist_toggle(&stale, "alpha", false).expect("second toggle persists");
        assert!(persistence.save_ok, "second writer must complete its save");
        McpConfig::load_from_file(&second_path).expect("read after second writer")
    });
    std::thread::sleep(std::time::Duration::from_millis(200));

    let mut first_saved = stale.clone();
    first_saved
        .servers
        .insert("beta".to_string(), server_cfg("beta", false).1);
    first_saved.save_to_file(&path).expect("first writer saves");
    drop(first);

    let saved = second.join().expect("probe thread must not panic");
    let waited = before.elapsed();
    assert!(
        waited >= std::time::Duration::from_millis(200),
        "second writer must block behind the held lock, finished in {waited:?}"
    );
    assert_eq!(saved.servers["beta"].enabled, Some(false));
    assert_eq!(saved.servers["alpha"].enabled, Some(false));
    drop(home);
}
