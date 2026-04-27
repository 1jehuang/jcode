use super::{
    ClientCandidate, extract_session_short_name_from_window_title, last_focused_session,
    normalize_session_short_name, parse_ppid, read_resumed_session_id,
    remember_last_focused_session, run_command, select_candidate,
};
#[cfg(target_os = "linux")]
use super::focused_jcode_session;
#[cfg(target_os = "linux")]
use std::ffi::OsString;

#[cfg(target_os = "linux")]
struct EnvVarGuard {
    key: &'static str,
    previous: Option<OsString>,
}

#[cfg(target_os = "linux")]
impl EnvVarGuard {
    fn set<K: AsRef<std::ffi::OsStr>>(key: &'static str, value: K) -> Self {
        let previous = std::env::var_os(key);
        crate::env::set_var(key, value);
        Self { key, previous }
    }
}

#[cfg(target_os = "linux")]
impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        if let Some(previous) = &self.previous {
            crate::env::set_var(self.key, previous);
        } else {
            crate::env::remove_var(self.key);
        }
    }
}

#[cfg(target_os = "linux")]
struct ChildGuard(std::process::Child);

#[cfg(target_os = "linux")]
impl ChildGuard {
    fn spawn_named(name: &str) -> Self {
        let child = std::process::Command::new("python3")
                .args([
                    "-c",
                    "import ctypes, sys, time; libc = ctypes.CDLL(None); libc.prctl(15, sys.argv[1].encode(), 0, 0, 0); time.sleep(30)",
                    name,
                ])
                .spawn()
                .expect("spawn named helper process");
        Self(child)
    }

    fn pid(&self) -> u32 {
        self.0.id()
    }
}

#[cfg(target_os = "linux")]
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[cfg(target_os = "linux")]
fn install_fake_niri(bin_dir: &std::path::Path, pid: u32, title: &str) {
    use std::os::unix::fs::PermissionsExt;

    std::fs::create_dir_all(bin_dir).expect("create fake bin dir");
    let script = bin_dir.join("niri");
    let json = serde_json::json!({
        "pid": pid,
        "title": title,
        "app_id": "kitty"
    });
    std::fs::write(&script, format!("#!/bin/sh\nprintf '%s\\n' '{}'\n", json))
        .expect("write fake niri script");
    let mut perms = std::fs::metadata(&script)
        .expect("fake niri metadata")
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&script, perms).expect("chmod fake niri");
}

#[test]
fn parse_ppid_from_proc_status() {
    let status = "Name:\tbash\nState:\tS (sleeping)\nPPid:\t1234\n";
    assert_eq!(parse_ppid(status), Some(1234));
}

#[tokio::test]
async fn run_command_trims_trailing_newlines() {
    let text = run_command("printf 'hello from test\\n'", 5)
        .await
        .expect("dictation command should succeed");
    assert_eq!(text, "hello from test");
}

#[test]
fn select_candidate_prefers_title_match() {
    let candidates = vec![
        ClientCandidate {
            pid: 1,
            short_name: "whale".to_string(),
            session_id: Some("session_whale_1".to_string()),
        },
        ClientCandidate {
            pid: 2,
            short_name: "crab".to_string(),
            session_id: Some("session_crab_1".to_string()),
        },
    ];

    let selected = select_candidate(&candidates, Some("🦀 jcode/sleeping Crab [self-dev]"))
        .expect("should select matching candidate");
    assert_eq!(selected.short_name, "crab");
}

#[test]
fn read_resumed_session_id_from_cmdline_for_current_process() {
    let _ = read_resumed_session_id(std::process::id());
}

#[test]
fn extract_session_short_name_from_jcode_window_title() {
    assert_eq!(
        extract_session_short_name_from_window_title("🦢 jcode/cliff Swan [self-dev]"),
        Some("swan".to_string())
    );
    assert_eq!(
        extract_session_short_name_from_window_title("🦊 jcode Fox"),
        Some("fox".to_string())
    );
}

#[test]
fn normalize_session_short_name_strips_wrapping_punctuation() {
    assert_eq!(
        normalize_session_short_name("[Swan]"),
        Some("swan".to_string())
    );
    assert_eq!(
        normalize_session_short_name("Polar-bear"),
        Some("polar-bear".to_string())
    );
}

#[test]
fn remember_and_read_last_focused_session() {
    let _guard = crate::storage::lock_test_env();
    let prev = std::env::var_os("JCODE_HOME");
    let temp = tempfile::TempDir::new().expect("tempdir");
    crate::env::set_var("JCODE_HOME", temp.path());

    let active_dir = temp.path().join("active_pids");
    std::fs::create_dir_all(&active_dir).expect("create active_pids");
    std::fs::write(active_dir.join("session_whale_123"), "99999").expect("write active pid");

    remember_last_focused_session("session_whale_123").expect("remember session");
    assert_eq!(
        last_focused_session().expect("read session"),
        Some("session_whale_123".to_string())
    );

    if let Some(prev) = prev {
        crate::env::set_var("JCODE_HOME", prev);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[cfg(target_os = "linux")]
#[test]
fn focused_jcode_session_uses_niri_window_title_when_process_name_is_generic() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().expect("tempdir");
    let _home = EnvVarGuard::set("JCODE_HOME", temp.path());

    let active_dir = temp.path().join("active_pids");
    std::fs::create_dir_all(&active_dir).expect("create active_pids");
    std::fs::write(active_dir.join("session_swan_123"), "12345").expect("write active pid");

    let focused_process = ChildGuard::spawn_named("jcode");
    let bin_dir = temp.path().join("bin");
    install_fake_niri(
        &bin_dir,
        focused_process.pid(),
        "🦢 jcode/cliff Swan [self-dev]",
    );

    let prev_path = std::env::var_os("PATH").unwrap_or_default();
    let mut path = OsString::from(bin_dir.as_os_str());
    path.push(":");
    path.push(prev_path);
    let _path = EnvVarGuard::set("PATH", path);

    assert_eq!(
        focused_jcode_session().expect("resolve focused session"),
        Some("session_swan_123".to_string())
    );
}
