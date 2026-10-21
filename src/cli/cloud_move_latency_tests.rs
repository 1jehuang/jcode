//! Each environment-dependent case runs in its own test process. Never mutate
//! PATH, HOME, or JCODE_HOME in the shared test runner.
use super::*;
use std::os::unix::fs::PermissionsExt;

const CHILD_MODE: &str = "JCODE_CLOUD_LATENCY_TEST_MODE";
const CHILD_ROOT: &str = "JCODE_CLOUD_LATENCY_TEST_ROOT";

fn bundle_fixture(root: &Path, stage: &Path) -> (RepoManifest, String) {
    git(root, &["init", "--quiet"]).unwrap();
    git(root, &["config", "user.name", "Latency Test"]).unwrap();
    git(root, &["config", "user.email", "latency@example.invalid"]).unwrap();
    git(root, &["commit", "--quiet", "--allow-empty", "-m", "base"]).unwrap();
    let base = git(root, &["rev-parse", "HEAD"]).unwrap();
    git(root, &["commit", "--quiet", "--allow-empty", "-m", "head"]).unwrap();
    std::fs::write(root.join("uncommitted.txt"), "local work\n").unwrap();
    let repo = prepare_repo_snapshot(root, "bundle-test", stage)
        .unwrap()
        .unwrap();
    (repo, base)
}

#[test]
fn incremental_bundle_preserves_branch_and_full_wrapper_is_self_contained() {
    let root = tempfile::tempdir().unwrap();
    let stage = tempfile::tempdir().unwrap();
    let empty = tempfile::tempdir().unwrap();
    git(empty.path(), &["init", "--quiet"]).unwrap();
    let (repo, base) = bundle_fixture(root.path(), stage.path());
    assert!(
        !stage.path().join("repo.bundle").exists(),
        "preparation must not pack history"
    );
    create_repo_bundle(&repo, "bundle-test", stage.path(), Some(&base)).unwrap();
    let bundle = stage.path().join("repo.bundle");
    let bundle = bundle.to_str().unwrap();
    let refs = git(root.path(), &["bundle", "list-heads", bundle]).unwrap();
    assert!(
        refs.contains(&format!(
            "{} refs/heads/{}",
            repo.head,
            repo.branch.as_ref().unwrap()
        )),
        "{refs}"
    );
    git(root.path(), &["bundle", "verify", bundle]).unwrap();
    assert!(
        git(empty.path(), &["bundle", "verify", bundle]).is_err(),
        "incremental bundle must require base"
    );

    // Excluding HEAD would hide the advertised branch from older receivers.
    // Unsafe and unknown bases must therefore fall back to a full bundle.
    for unsafe_base in [
        repo.head.as_str(),
        repo.snapshot.as_str(),
        "0000000000000000000000000000000000000000",
    ] {
        create_repo_bundle(&repo, "bundle-test", stage.path(), Some(unsafe_base)).unwrap();
        git(empty.path(), &["bundle", "verify", bundle]).unwrap();
    }
    let full = tempfile::tempdir().unwrap();
    let before = git(root.path(), &["status", "--porcelain"]).unwrap();
    snapshot_repo(root.path(), "full-wrapper", full.path())
        .unwrap()
        .unwrap();
    git(
        empty.path(),
        &[
            "bundle",
            "verify",
            full.path().join("repo.bundle").to_str().unwrap(),
        ],
    )
    .unwrap();
    assert_eq!(
        git(root.path(), &["status", "--porcelain"]).unwrap(),
        before
    );
}

fn executable(path: &Path, script: &str) {
    std::fs::write(path, script).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn isolated_preflight_and_parallel_preparation_regressions() {
    for mode in [
        "ssh",
        "ssh-failure",
        "transport",
        "transport-failure",
        "prepare",
        "prepare-failure",
        "overlap",
        "join-error",
        "history",
        "history-unknown",
    ] {
        let root = tempfile::tempdir().unwrap();
        let bin = root.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        // Fake SSH accepts the same final shell-command argument as real SSH.
        executable(
            &bin.join("ssh"),
            "#!/bin/sh\nprintf 'ssh\\n' >> \"$JCODE_CLOUD_LATENCY_TEST_ROOT/calls\"\ncase \"$JCODE_CLOUD_LATENCY_TEST_MODE\" in history*) cat \"$JCODE_CLOUD_LATENCY_TEST_ROOT/remote-history\"; exit 0;; esac\nfor arg do command=$arg; done\nexec /bin/sh -c \"$command\"\n",
        );
        executable(
            &root.path().join("transport"),
            "#!/bin/sh\nprintf 'transport\\n' >> \"$JCODE_CLOUD_LATENCY_TEST_ROOT/calls\"\nexec /bin/sh -c \"$1\"\n",
        );
        // The remote binary path deliberately contains spaces and a quote.
        executable(
            &root.path().join("remote ' binary"),
            r#"#!/bin/sh
printf '%s\n' "$*" >> "$JCODE_CLOUD_LATENCY_TEST_ROOT/probes"
case "$JCODE_CLOUD_LATENCY_TEST_MODE" in
  *failure)
    if [ "$4" = credentials ]; then
      echo 'unrecognized subcommand credentials' >&2
      exit 9
    fi
    ;;
  overlap)
    n=0
    while [ ! -f "$JCODE_CLOUD_LATENCY_TEST_ROOT/local-started" ]; do
      n=$((n + 1))
      if [ "$n" -ge 100 ]; then echo 'local preparation did not overlap' >&2; exit 9; fi
      sleep 0.02
    done
    touch "$JCODE_CLOUD_LATENCY_TEST_ROOT/overlapped"
    echo 'intentional preflight stop' >&2
    exit 9
    ;;
  join-error)
    sleep 0.1
    touch "$JCODE_CLOUD_LATENCY_TEST_ROOT/preflight-finished"
    echo 'intentional preflight stop' >&2
    exit 9
    ;;
esac
"#,
        );
        // A marker at the first local repository probe proves that preparation
        // can proceed while the remote protocol probe is still waiting.
        executable(
            &bin.join("git"),
            "#!/bin/sh\ncase \"$JCODE_CLOUD_LATENCY_TEST_MODE\" in history*) exec \"$JCODE_CLOUD_REAL_GIT\" \"$@\";; esac\ntouch \"$JCODE_CLOUD_LATENCY_TEST_ROOT/local-started\"\nexit 1\n",
        );
        let path = std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        )))
        .unwrap();
        let real_git = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
            .map(|directory| directory.join("git"))
            .find(|candidate| candidate.is_file())
            .expect("git available for tests");
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "cli::cloud_move::latency_tests::isolated_latency_child",
                "--nocapture",
            ])
            .env(CHILD_MODE, mode)
            .env("JCODE_CLOUD_REAL_GIT", real_git)
            .env(CHILD_ROOT, root.path())
            .env("JCODE_HOME", root.path().join("home"))
            .env("JCODE_SESSION_ID", "latency-session")
            .env("PATH", path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{mode}:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            root.path().join("child-ran").exists(),
            "child test filter did not match"
        );
    }
}

#[test]
fn isolated_latency_child() {
    let Ok(mode) = std::env::var(CHILD_MODE) else {
        return;
    };
    let root = PathBuf::from(std::env::var_os(CHILD_ROOT).unwrap());
    std::fs::write(root.join("child-ran"), "").unwrap();
    let target = Target {
        host: Some("latency-test.invalid".into()),
        remote_binary: Some(root.join("remote ' binary").to_string_lossy().into_owned()),
        transport: mode
            .starts_with("transport")
            .then(|| root.join("transport").to_string_lossy().into_owned()),
    };
    if mode.starts_with("history") {
        let checkout = root.join("checkout");
        std::fs::create_dir(&checkout).unwrap();
        let stage = tempfile::tempdir().unwrap();
        let (repo, base) = bundle_fixture(&checkout, stage.path());
        let unknown = "0000000000000000000000000000000000000000";
        // Unknown cloud-only tip, then known HEAD (unsafe to exclude), then
        // a shared strict ancestor. Neither unknown nor HEAD may be selected.
        let history = if mode == "history" {
            format!("{unknown}\n{}\n{base}\n", repo.head)
        } else {
            format!("{unknown}\n")
        };
        std::fs::write(root.join("remote-history"), history).unwrap();
        let common = common_remote_commit(&target, "latency-test.invalid", &repo);
        assert_eq!(common, (mode == "history").then_some(base));
        let custom = Target {
            transport: Some("unused".into()),
            ..target.clone()
        };
        assert_eq!(
            common_remote_commit(&custom, "latency-test.invalid", &repo),
            None
        );
        let calls = std::fs::read_to_string(root.join("calls")).unwrap();
        assert_eq!(
            calls.lines().count(),
            1,
            "custom transport must not probe Git"
        );
        return;
    }
    if mode == "overlap" || mode == "join-error" {
        let mut sess = session::Session::create_with_id("latency-session".into(), None, None);
        sess.working_dir = Some(root.to_string_lossy().into_owned());
        if mode == "join-error" {
            sess.route_api_method = Some("openai-oauth@".into());
        }
        sess.save_prepared().unwrap();
        let error = format!("{:#}", run_move(None, &target, true, true).unwrap_err());
        assert!(error.contains("intentional preflight stop"), "{error}");
        if mode == "overlap" {
            assert!(
                root.join("overlapped").exists(),
                "preflight was not overlapped"
            );
        } else {
            assert!(
                root.join("preflight-finished").exists(),
                "preflight was not joined"
            );
            assert!(
                error.contains("invalid saved OAuth account route"),
                "{error}"
            );
        }
        let calls = std::fs::read_to_string(root.join("calls")).unwrap();
        assert_eq!(
            calls.lines().count(),
            1,
            "must not upload after preflight failure"
        );
        return;
    }
    let result = if mode.starts_with("prepare") {
        let mut sess = session::Session::create_with_id("latency-session".into(), None, None);
        sess.working_dir = Some(root.to_string_lossy().into_owned());
        sess.save_prepared().unwrap();
        storage::mark_streaming("latency-session");
        assert!(storage::streaming_session_ids().contains(&"latency-session".to_string()));
        let path = session::session_path("latency-session").unwrap();
        let before = std::fs::read(&path).unwrap();
        let result = prepare_host(&target);
        assert_eq!(
            std::fs::read(&path).unwrap(),
            before,
            "preparation must not change the transcript"
        );
        assert!(storage::read_session_lease("latency-session").is_none());
        assert!(
            !root.join("local-started").exists(),
            "preparation must not snapshot the repository"
        );
        assert!(
            !baseline_path("latency-session").unwrap().exists(),
            "preparation must not transfer credentials"
        );
        storage::unmark_streaming("latency-session");
        result.map(|host| assert_eq!(host, "latency-test.invalid"))
    } else {
        target.preflight("latency-test.invalid")
    };
    if mode.ends_with("failure") {
        let error = format!("{:#}", result.unwrap_err());
        assert!(error.contains("complete cloud handoff protocol"), "{error}");
        assert!(error.contains("credentials"), "{error}");
    } else {
        result.unwrap();
    }
    let probes = std::fs::read_to_string(root.join("probes")).unwrap();
    let expected = if mode.ends_with("failure") { 2 } else { 3 };
    assert_eq!(probes.lines().count(), expected, "{probes}");
    assert!(
        probes
            .lines()
            .next()
            .unwrap()
            .ends_with("cloud receive --help")
    );
    assert!(
        probes
            .lines()
            .nth(1)
            .unwrap()
            .ends_with("cloud credentials --help")
    );
    if expected == 3 {
        assert!(
            probes
                .lines()
                .nth(2)
                .unwrap()
                .ends_with("cloud cancel-return --help")
        );
    }
    let calls = std::fs::read_to_string(root.join("calls")).unwrap();
    assert_eq!(
        calls.lines().count(),
        if mode.starts_with("transport") {
            expected
        } else {
            1
        }
    );
}
