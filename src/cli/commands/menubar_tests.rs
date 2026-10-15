use super::*;
use crate::session::SessionCounts;

#[test]
fn title_no_sessions_is_icon_only() {
    let title = format_menubar_title(SessionCounts {
        total: 0,
        streaming: 0,
    });
    assert_eq!(title, "");
}

#[test]
fn title_idle_shows_zero_streaming_and_total() {
    let title = format_menubar_title(SessionCounts {
        total: 5,
        streaming: 0,
    });
    assert_eq!(title, "0/5");
}

#[test]
fn title_streaming_shows_compact_ratio() {
    let title = format_menubar_title(SessionCounts {
        total: 7,
        streaming: 2,
    });
    assert_eq!(title, "2/7");
}

#[test]
fn summary_pluralizes_sessions() {
    assert_eq!(
        format_menubar_summary(SessionCounts {
            total: 1,
            streaming: 0,
        }),
        "0 streaming · 1 session running"
    );
    assert_eq!(
        format_menubar_summary(SessionCounts {
            total: 3,
            streaming: 1,
        }),
        "1 streaming · 3 sessions running"
    );
}

#[test]
fn session_menu_item_title_uses_meaningful_title_instead_of_animal_name() {
    let session_id = "session_buffalo_1781229104969_6d487ff77287de4f";
    assert_eq!(
        format_session_menu_item_title_with_display(session_id, Some("Improve menu labels"), false),
        "🐃 Improve menu labels"
    );
    assert_eq!(
        format_session_menu_item_title_with_display(session_id, Some("Improve menu labels"), true),
        "🐃 Improve menu labels · streaming"
    );
}

#[test]
fn session_menu_item_title_is_icon_only_without_meaningful_title() {
    let session_id = "session_buffalo_1781229104969_6d487ff77287de4f";
    assert_eq!(
        format_session_menu_item_title_with_display(session_id, None, false),
        "🐃"
    );
    assert_eq!(
        format_session_menu_item_title_with_display("weird-id", None, false),
        "💫"
    );
}

#[test]
fn session_menu_item_title_loads_persisted_todo_title() {
    let _guard = crate::storage::lock_test_env();
    let previous_home = std::env::var_os("JCODE_HOME");
    let temp = tempfile::tempdir().expect("create temporary JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());

    let session_id = "session_buffalo_1781229104969_6d487ff77287de4f";
    let mut session = crate::session::Session::create_with_id(
        session_id.to_string(),
        None,
        Some("Generated session title".to_string()),
    );
    session.save().expect("save session");
    crate::todo::save_todos(
        session_id,
        &[crate::todo::TodoItem {
            content: "Use todo context in the menu bar".to_string(),
            status: "in_progress".to_string(),
            priority: "high".to_string(),
            id: "menubar-label".to_string(),
            group: Some("Meaningful menu labels".to_string()),
            confidence: Some(crate::todo::ConfidenceState::Plausible),
            completion_confidence: None,
            confidence_history: Vec::new(),
            blocked_by: Vec::new(),
            assigned_to: None,
        }],
    )
    .expect("save todos");

    assert_eq!(
        format_session_menu_item_title(session_id, false),
        "🐃 Meaningful menu labels"
    );

    if let Some(previous_home) = previous_home {
        crate::env::set_var("JCODE_HOME", previous_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn menu_presence_includes_only_user_root_sessions() {
    let _guard = crate::storage::lock_test_env();
    let previous_home = std::env::var_os("JCODE_HOME");
    let temp = tempfile::tempdir().expect("create temporary JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());

    let mut root = crate::session::Session::create_with_id(
        "session_fox_root".to_string(),
        None,
        Some("Root work".to_string()),
    );
    root.mark_active();
    root.save().expect("save root session");

    let mut debug = crate::session::Session::create_with_id(
        "session_owl_debug".to_string(),
        None,
        Some("Internal worker".to_string()),
    );
    debug.set_debug(true);
    debug.mark_active();
    debug.save().expect("save debug session");

    let mut child = crate::session::Session::create_with_id(
        "session_bear_child".to_string(),
        Some(root.id.clone()),
        Some("Child work".to_string()),
    );
    child.mark_active();
    child.save().expect("save child session");

    let visible = user_root_session_presence();
    assert_eq!(
        visible
            .iter()
            .map(|presence| presence.session_id.as_str())
            .collect::<Vec<_>>(),
        vec!["session_fox_root"]
    );
    assert_eq!(counts_for_presence(&visible).total, 1);

    root.mark_closed();
    debug.mark_closed();
    child.mark_closed();
    if let Some(previous_home) = previous_home {
        crate::env::set_var("JCODE_HOME", previous_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn counts_report_serializes_to_json() {
    let report = CountsReport::from(SessionCounts {
        total: 4,
        streaming: 2,
    });
    let json = serde_json::to_string(&report).unwrap();
    assert_eq!(json, r#"{"total":4,"streaming":2}"#);
}

#[cfg(target_os = "macos")]
#[test]
fn menubar_sandbox_detection() {
    use std::ffi::OsStr;
    use std::path::Path;

    let real = Path::new("/Users/me");
    let home = Some(OsStr::new("/Users/me"));

    // Real user, no override: not a sandbox -> owns the menu bar.
    assert!(!is_menubar_sandbox(false, false, None, home, Some(real)));
    assert!(!is_menubar_sandbox(false, false, None, None, Some(real)));
    // Override equal to the real home is still the real user.
    assert!(!is_menubar_sandbox(
        false,
        false,
        Some(OsStr::new("/Users/me/.jcode")),
        home,
        Some(real),
    ));

    // Explicit test/temp markers force sandbox regardless of home.
    assert!(is_menubar_sandbox(true, false, None, home, Some(real)));
    assert!(is_menubar_sandbox(false, true, None, home, Some(real)));

    // A throwaway sandbox home (e2e / self-dev / onboarding) is a sandbox.
    assert!(is_menubar_sandbox(
        false,
        false,
        Some(OsStr::new("/private/tmp/jcode-e2e-home-xyz")),
        home,
        Some(real),
    ));

    // No discoverable real home: any explicit override is treated as sandbox.
    assert!(is_menubar_sandbox(
        false,
        false,
        Some(OsStr::new("/private/tmp/jcode-e2e-home-xyz")),
        home,
        None,
    ));
}

#[cfg(target_os = "macos")]
#[test]
fn menubar_sandbox_detects_consistent_home_override() {
    use std::path::Path;

    // HOME and JCODE_HOME both overridden consistently (the harness case
    // that produced duplicate status items) must be a sandbox.
    let tmp = tempfile::tempdir().unwrap();
    let fake_home = tmp.path().join("home");
    std::fs::create_dir_all(fake_home.join(".jcode")).unwrap();
    let real = Path::new("/Users/me");
    assert!(is_menubar_sandbox(
        false,
        false,
        Some(fake_home.join(".jcode").as_os_str()),
        Some(fake_home.as_os_str()),
        Some(real),
    ));
    // Foreign HOME with no JCODE_HOME override is also a sandbox.
    assert!(is_menubar_sandbox(
        false,
        false,
        None,
        Some(fake_home.as_os_str()),
        Some(real),
    ));
}

#[cfg(target_os = "macos")]
#[test]
fn menubar_sandbox_accepts_symlinked_real_home() {
    let tmp = tempfile::tempdir().unwrap();
    let real = tmp.path().join("real");
    std::fs::create_dir_all(real.join(".jcode")).unwrap();
    let link = tmp.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert!(!is_menubar_sandbox(
        false,
        false,
        Some(link.join(".jcode").as_os_str()),
        Some(link.as_os_str()),
        Some(&real),
    ));
}

#[cfg(target_os = "macos")]
#[test]
fn menubar_sandbox_fails_closed_without_passwd_home() {
    use std::ffi::OsStr;
    // passwd lookup failed (real_home == None): always a sandbox, with or
    // without HOME / JCODE_HOME overrides, so no helper and no lock split.
    assert!(is_menubar_sandbox(false, false, None, None, None));
    assert!(is_menubar_sandbox(
        false,
        false,
        None,
        Some(OsStr::new("/tmp/x")),
        None
    ));
    assert!(is_menubar_sandbox(
        false,
        false,
        Some(OsStr::new("/tmp/x/.jcode")),
        Some(OsStr::new("/tmp/x")),
        None,
    ));
}

fn menubar_args_fixture(exe: &[u8], argv: &[&[u8]]) -> Vec<u8> {
    let mut bytes = (argv.len() as i32).to_ne_bytes().to_vec();
    bytes.extend_from_slice(exe);
    bytes.extend_from_slice(&[0, 0, 0]);
    for arg in argv {
        bytes.extend_from_slice(arg);
        bytes.push(0);
    }
    bytes.extend_from_slice(b"ENV=menubar\0");
    bytes
}

#[test]
fn menubar_helper_pids_matches_only_exact_menubar_argv() {
    let ps = "100\n101\n102\n103\n104\n105\n106\njunk\n0\n";
    let records = [
        (
            100,
            menubar_args_fixture(b"/build/jcode", &[b"/build/jcode", b"menubar"]),
        ),
        (
            101,
            menubar_args_fixture(b"/My Builds/jcode", &[b"/My Builds/jcode", b"menubar"]),
        ),
        (
            102,
            menubar_args_fixture(b"/build/jcode", &[b"jcode", b"serve"]),
        ),
        (
            103,
            menubar_args_fixture(b"/build/jcode", &[b"jcode", b"menubar", b"--x"]),
        ),
        (
            104,
            menubar_args_fixture(b"/bin/vim", &[b"jcode", b"menubar"]),
        ),
    ];
    assert_eq!(
        menubar_helper_pids(ps, 105, |pid| records
            .iter()
            .find(|(id, _)| *id == pid)
            .map(|(_, bytes)| bytes.clone())),
        vec![100, 101]
    );
}

#[test]
fn menubar_process_args_preserves_non_utf8_executable_paths() {
    assert!(is_menubar_process_args(&menubar_args_fixture(
        b"/My\xff Builds/jcode",
        &[b"jcode", b"menubar"]
    )));
}

#[test]
fn menubar_process_args_rejects_truncated_records() {
    let bytes = menubar_args_fixture(b"/My Builds/jcode", &[b"jcode", b"menubar"]);
    let required_len = bytes.len() - b"ENV=menubar\0".len();
    assert!((0..required_len).all(|len| !is_menubar_process_args(&bytes[..len])));
}

#[test]
fn menubar_process_args_rejects_combined_arguments() {
    assert!(!is_menubar_process_args(&menubar_args_fixture(
        b"/build/jcode",
        &[b"jcode", b"menubar --once"]
    )));
}

#[test]
fn menubar_process_args_rejects_empty_argument() {
    assert!(!is_menubar_process_args(&menubar_args_fixture(
        b"/build/jcode",
        &[b"jcode", b""]
    )));
}

#[test]
fn menubar_process_args_rejects_environment_only_menubar() {
    let mut bytes = menubar_args_fixture(b"/build/jcode", &[b"jcode"]);
    bytes.extend_from_slice(b"menubar\0");
    assert!(!is_menubar_process_args(&bytes));
}

#[test]
fn menubar_helper_pids_skips_self_before_reading_identity() {
    assert!(
        menubar_helper_pids("105\n0\njunk\n", 105, |_| panic!("unexpected process read"))
            .is_empty()
    );
}

#[cfg(target_os = "macos")]
#[test]
fn menubar_helper_discovery_finds_disposable_executable_with_spaces() {
    // This is a disposable link to cat, never Jcode or an AppKit helper.
    // It reads a private stdin pipe via a symlink named menubar and exits on EOF.
    // A copy of /bin/cat is sometimes SIGKILLed at launch by macOS, so link to it:
    // the kernel records the linked path, which is all discovery inspects.
    struct FixtureChild(std::process::Child);
    impl Drop for FixtureChild {
        fn drop(&mut self) {
            drop(self.0.stdin.take());
            let _ = self.0.wait();
        }
    }
    let dir = tempfile::Builder::new()
        .prefix("menubar discovery spaces ")
        .tempdir()
        .unwrap();
    let exe = dir.path().join("jcode-discovery-fixture");
    std::os::unix::fs::symlink("/bin/cat", &exe).unwrap();
    std::os::unix::fs::symlink("/dev/stdin", dir.path().join("menubar")).unwrap();
    let child = FixtureChild(
        std::process::Command::new(&exe)
            .arg("menubar")
            .current_dir(dir.path())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );
    // The process listing can trail the spawn slightly.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while !live_menubar_helper_pids().contains(&child.0.id()) {
        assert!(
            std::time::Instant::now() < deadline,
            "fixture helper never appeared in the process listing"
        );
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
}

const PASSWD_PROBE_ENV: &str = "JCODE_MENUBAR_PASSWD_PROBE";

/// Child-mode helper: a no-op unless spawned with PASSWD_PROBE_ENV set, in
/// which case it prints what `passwd_home_dir()` returns under the child's
/// overridden HOME.
#[cfg(unix)]
#[test]
fn passwd_home_probe_child() {
    if std::env::var_os(PASSWD_PROBE_ENV).is_none() {
        return;
    }
    let home = passwd_home_dir().expect("passwd home");
    println!("PASSWD_HOME={}", home.display());
}

#[cfg(unix)]
#[test]
fn passwd_home_matches_getpwuid_and_ignores_home_env() {
    use std::ffi::CStr;
    use std::os::unix::ffi::OsStrExt;
    // SAFETY: getpwuid returns a pointer to static storage; read immediately.
    let expected = unsafe {
        let pw = libc::getpwuid(libc::getuid());
        assert!(!pw.is_null());
        std::path::PathBuf::from(std::ffi::OsStr::from_bytes(
            CStr::from_ptr((*pw).pw_dir).to_bytes(),
        ))
    };
    let uid = unsafe { libc::getuid() };
    assert_eq!(passwd_home_dir_for_uid(uid).expect("passwd home"), expected);

    // Run the probe in a child with HOME pointing elsewhere. This process's
    // own HOME is never mutated (tests run in parallel).
    let fake_home = tempfile::tempdir().unwrap();
    assert_ne!(fake_home.path(), expected.as_path());
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "cli::commands::menubar::tests::passwd_home_probe_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(PASSWD_PROBE_ENV, "1")
        .env("HOME", fake_home.path())
        .output()
        .expect("spawn probe");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "probe failed: {stdout}");
    let got = stdout
        .lines()
        .find_map(|line| {
            let rest = &line[line.find("PASSWD_HOME=")? + "PASSWD_HOME=".len()..];
            Some(rest.trim_end())
        })
        .unwrap_or_else(|| panic!("probe printed no home: {stdout}"));
    assert_eq!(std::path::Path::new(got), expected.as_path());
}
