use super::*;
use crate::tests::with_temp_jcode_home;

#[test]
fn rejects_unsafe_version_labels() {
    for label in [
        "",
        ".",
        "..",
        ".hidden",
        "../escape",
        "a/b",
        "a\\b",
        " a",
        "a:",
        "a\n",
    ] {
        assert!(validate_version(label).is_err(), "{label:?}");
    }
}

#[test]
fn accepts_supported_version_labels() {
    for label in ["0.95.1", "main-abc123", "abc123-dirty-456", "v1.0.0+build"] {
        validate_version(label).unwrap();
    }
}

#[test]
fn stable_publication_preserves_current_channel() {
    with_temp_jcode_home(|| {
        let payload = tempfile::tempdir().unwrap();
        std::fs::write(payload.path().join(binary_name()), b"release").unwrap();
        let old = builds_dir().unwrap().join("versions/old");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join(binary_name()), b"old").unwrap();
        crate::update_current_symlink("old").unwrap();
        publish_prepared_build(
            payload.path(),
            "new",
            PreparedBuildChannel::Stable,
            &payload.path().join("launcher"),
            false,
        )
        .unwrap();
        assert_eq!(
            crate::read_current_version().unwrap().as_deref(),
            Some("old")
        );
    });
}

#[test]
fn publication_preserves_wrapper_payload_and_libraries() {
    with_temp_jcode_home(|| {
        let payload = tempfile::tempdir().unwrap();
        for (name, bytes) in [
            (binary_name(), "wrapper"),
            ("jcode-linux.bin", "payload"),
            ("libssl.so.3", "library"),
        ] {
            std::fs::write(payload.path().join(name), bytes).unwrap();
        }
        let result = publish_prepared_build(
            payload.path(),
            "new",
            PreparedBuildChannel::Current,
            &payload.path().join("launcher"),
            false,
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(result.parent().unwrap().join("libssl.so.3")).unwrap(),
            "library"
        );
        assert_eq!(
            std::fs::read_to_string(result.parent().unwrap().join("jcode-linux.bin")).unwrap(),
            "payload"
        );
    });
}

#[test]
fn missing_binary_fails_before_creating_version() {
    with_temp_jcode_home(|| {
        let payload = tempfile::tempdir().unwrap();
        assert!(
            publish_prepared_build(
                payload.path(),
                "new",
                PreparedBuildChannel::Stable,
                &payload.path().join("launcher"),
                false
            )
            .is_err()
        );
        assert!(!builds_dir().unwrap().join("versions/new").exists());
    });
}

#[test]
fn directories_in_prepared_payload_fail_before_creating_version() {
    with_temp_jcode_home(|| {
        let payload = tempfile::tempdir().unwrap();
        std::fs::write(payload.path().join(binary_name()), b"bin").unwrap();
        std::fs::create_dir(payload.path().join("unexpected")).unwrap();
        assert!(
            publish_prepared_build(
                payload.path(),
                "new",
                PreparedBuildChannel::Stable,
                &payload.path().join("launcher"),
                false
            )
            .is_err()
        );
        assert!(!builds_dir().unwrap().join("versions/new").exists());
    });
}

#[test]
fn identical_republication_reuses_immutable_payload() {
    with_temp_jcode_home(|| {
        let payload = tempfile::tempdir().unwrap();
        let launcher = tempfile::tempdir().unwrap();
        std::fs::write(payload.path().join(binary_name()), b"release").unwrap();
        let installed = publish_prepared_build(
            payload.path(),
            "new",
            PreparedBuildChannel::Stable,
            &launcher.path().join(binary_name()),
            false,
        )
        .unwrap();
        let before = std::fs::metadata(&installed).unwrap().modified().unwrap();
        publish_prepared_build(
            payload.path(),
            "new",
            PreparedBuildChannel::Stable,
            &launcher.path().join(binary_name()),
            false,
        )
        .unwrap();
        assert_eq!(
            std::fs::metadata(installed).unwrap().modified().unwrap(),
            before
        );
    });
}

#[test]
fn differing_republication_preserves_existing_payload_and_channels() {
    with_temp_jcode_home(|| {
        let payload = tempfile::tempdir().unwrap();
        let launcher = tempfile::tempdir().unwrap();
        std::fs::write(payload.path().join(binary_name()), b"release").unwrap();
        let installed = publish_prepared_build(
            payload.path(),
            "old",
            PreparedBuildChannel::Current,
            &launcher.path().join(binary_name()),
            false,
        )
        .unwrap();
        std::fs::write(payload.path().join(binary_name()), b"replacement").unwrap();
        assert!(
            publish_prepared_build(
                payload.path(),
                "old",
                PreparedBuildChannel::Current,
                &launcher.path().join(binary_name()),
                false
            )
            .is_err()
        );
        assert_eq!(std::fs::read(installed).unwrap(), b"release");
        assert_eq!(
            crate::read_current_version().unwrap().as_deref(),
            Some("old")
        );
    });
}

#[test]
fn source_inside_another_installed_version_is_rejected() {
    with_temp_jcode_home(|| {
        let source = builds_dir().unwrap().join("versions/source");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join(binary_name()), b"release").unwrap();
        assert!(
            publish_prepared_build(
                &source,
                "new",
                PreparedBuildChannel::Stable,
                &source.join("launcher"),
                false
            )
            .is_err()
        );
        assert!(!builds_dir().unwrap().join("versions/new").exists());
    });
}

#[test]
fn same_file_comparison_checks_content_and_length() {
    let fixture = tempfile::tempdir().unwrap();
    let left = fixture.path().join("left");
    let right = fixture.path().join("right");
    std::fs::write(&left, vec![b'a'; 32769]).unwrap();
    std::fs::write(&right, vec![b'a'; 32769]).unwrap();
    assert!(same_file_contents(&left, &right).unwrap());
    std::fs::write(&right, vec![b'b'; 32769]).unwrap();
    assert!(!same_file_contents(&left, &right).unwrap());
    std::fs::write(&right, b"short").unwrap();
    assert!(!same_file_contents(&left, &right).unwrap());
}

#[cfg(unix)]
mod processes {
    use super::*;
    use std::io::Write;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};

    const HELPER: &str = "publication::tests::processes::store_process_helper";

    struct Process(Child);
    impl Drop for Process {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    impl Process {
        fn finish(&mut self) {
            let started = Instant::now();
            loop {
                if let Some(status) = self.0.try_wait().unwrap() {
                    assert!(status.success(), "child failed: {status}");
                    return;
                }
                assert!(
                    started.elapsed() < Duration::from_secs(20),
                    "child did not finish"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }

    fn accept(listener: &UnixListener, message: &[u8]) -> UnixStream {
        listener.set_nonblocking(true).unwrap();
        let started = Instant::now();
        loop {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let mut received = vec![0; message.len()];
                    super::super::read_exact_bounded(&mut stream, &mut received).unwrap();
                    assert_eq!(received, message);
                    return stream;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        started.elapsed() < Duration::from_secs(15),
                        "barrier never reached"
                    );
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("accept: {error}"),
            }
        }
    }

    fn helper(home: &Path, action: &str) -> Command {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", HELPER, "--nocapture"])
            .env("JCODE_HOME", home)
            .env("JCODE_TEST_PROCESS_ACTION", action)
            .env_remove("JCODE_TEST_PUBLICATION_SOCKET")
            .env_remove("JCODE_TEST_LOCK_ATTEMPT_SOCKET")
            .stdin(Stdio::null());
        command
    }

    #[test]
    fn store_process_helper() {
        let Ok(action) = std::env::var("JCODE_TEST_PROCESS_ACTION") else {
            return;
        };
        // Child helpers MUST have a fresh disposable store supplied by parent.
        assert!(std::env::var_os("JCODE_HOME").is_some());
        match action.as_str() {
            "publish" => {
                let source = PathBuf::from(std::env::var_os("JCODE_TEST_PUBLISH_SOURCE").unwrap());
                let launcher =
                    PathBuf::from(std::env::var_os("JCODE_TEST_PUBLISH_LAUNCHER").unwrap());
                let version = std::env::var("JCODE_TEST_PUBLISH_VERSION").unwrap();
                let channel = match std::env::var("JCODE_TEST_PUBLISH_CHANNEL")
                    .unwrap()
                    .as_str()
                {
                    "current" => PreparedBuildChannel::Current,
                    "stable" => PreparedBuildChannel::Stable,
                    other => panic!("unexpected channel {other}"),
                };
                publish_prepared_build(&source, &version, channel, &launcher, false).unwrap();
            }
            "prune" => crate::run_prune_builds_command(Some(0), false).unwrap(),
            "lock" => {
                let _guard = acquire_build_store_lock().unwrap();
                super::super::publication_barrier().unwrap();
            }
            _ => panic!("unexpected process action {action}"),
        }
    }

    fn executable(path: &Path, contents: &str) {
        std::fs::write(path, contents).unwrap();
        crate::platform_support::set_permissions_executable(path).unwrap();
    }

    // Execute the actual installer, stubbing only cargo/git and its newly built
    // executable's CLI boundary. The helper calls the production transaction,
    // not a copied publisher. Both publication and pruning run in OS processes.
    #[test]
    fn actual_source_installer_blocks_keep_zero_pruner_until_reference_publication() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path();
        let home = root.join("home store");
        let repo = root.join("repo with spaces");
        let tools = root.join("tools path");
        let launcher = root.join("bin path/jcode");
        std::fs::create_dir_all(repo.join("target/release")).unwrap();
        std::fs::create_dir_all(&tools).unwrap();
        let old = home.join("builds/versions/old-idle");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join(binary_name()), b"old").unwrap();
        executable(&tools.join("cargo"), "#!/bin/sh\nexit 0\n");
        executable(
            &tools.join("git"),
            "#!/bin/sh\nif [ \"$1\" = rev-parse ]; then pwd; else exit 1; fi\n",
        );
        executable(&tools.join("date"), "#!/bin/sh\nprintf 'fixture-new\\n'\n");
        executable(
            &repo.join("target/release/jcode"),
            &format!(
                r#"#!/bin/sh
if [ "$1" = --no-update ]; then shift; fi
case "$1" in
publish-build)
  shift
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --source) export JCODE_TEST_PUBLISH_SOURCE="$2"; shift 2 ;;
      --version) export JCODE_TEST_PUBLISH_VERSION="$2"; shift 2 ;;
      --channel) export JCODE_TEST_PUBLISH_CHANNEL="$2"; shift 2 ;;
      --launcher) export JCODE_TEST_PUBLISH_LAUNCHER="$2"; shift 2 ;;
      *) exit 2 ;;
    esac
  done
  export JCODE_TEST_PROCESS_ACTION=publish
  exec "$JCODE_TEST_HELPER" --exact {HELPER} --nocapture
  ;;
*) exit 0 ;;
esac
"#
            ),
        );
        // Keep socket paths short enough for macOS sockaddr_un.
        let sockets = tempfile::tempdir().unwrap();
        let publication_socket = sockets.path().join("pub.sock");
        let prune_socket = sockets.path().join("prune.sock");
        let publication_listener = UnixListener::bind(&publication_socket).unwrap();
        let prune_listener = UnixListener::bind(&prune_socket).unwrap();
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/install_release.sh");
        let mut installer = Process(
            Command::new("bash")
                .arg(script)
                .arg("--fast")
                .current_dir(&repo)
                .env(
                    "PATH",
                    format!("{}:{}", tools.display(), std::env::var("PATH").unwrap()),
                )
                .env("HOME", &home)
                .env("JCODE_HOME", &home)
                .env("JCODE_INSTALL_DIR", launcher.parent().unwrap())
                .env("JCODE_TEST_HELPER", std::env::current_exe().unwrap())
                .env("JCODE_TEST_PUBLICATION_SOCKET", &publication_socket)
                .env("JCODE_SKIP_SERVER_RELOAD", "1")
                .env("JCODE_KEEP_BUILD_VERSIONS", "0")
                .stdin(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let mut publication = accept(&publication_listener, b"copied");
        let new_binary = home.join("builds/versions/fixture-new/jcode");
        assert!(
            new_binary.is_file(),
            "must pause in rename-to-reference gap"
        );
        assert!(!home.join("builds/stable-version").exists());
        let mut pruner = Process(
            helper(&home, "prune")
                .env("JCODE_TEST_LOCK_ATTEMPT_SOCKET", &prune_socket)
                .spawn()
                .unwrap(),
        );
        let _blocked = accept(&prune_listener, b"blocked");
        assert!(
            old.is_dir(),
            "pruner must not delete while publication owns lock"
        );
        publication.write_all(b"!").unwrap();
        installer.finish();
        pruner.finish();
        assert!(new_binary.is_file());
        assert!(
            launcher.exists(),
            "launcher must not dangle after keep zero"
        );
        assert!(
            !old.exists(),
            "keep zero must still prune unreferenced old builds"
        );
    }

    #[test]
    fn killing_lock_owner_recovers_without_removing_lock_file() {
        let fixture = tempfile::tempdir().unwrap();
        let sockets = tempfile::tempdir().unwrap();
        let socket = sockets.path().join("lock.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let mut owner = Process(
            helper(fixture.path(), "lock")
                .env("JCODE_TEST_PUBLICATION_SOCKET", &socket)
                .spawn()
                .unwrap(),
        );
        let _paused = accept(&listener, b"copied");
        owner.0.kill().unwrap();
        owner.0.wait().unwrap();
        let _guard =
            crate::store_lock::acquire_in(&fixture.path().join("builds"), Duration::ZERO).unwrap();
        assert!(fixture.path().join("builds/.store.lock").is_file());
    }

    #[test]
    fn symlink_version_directory_does_not_escape_the_store() {
        with_temp_jcode_home(|| {
            let payload = tempfile::tempdir().unwrap();
            let outside = tempfile::tempdir().unwrap();
            std::fs::write(payload.path().join(binary_name()), b"release").unwrap();
            let versions = builds_dir().unwrap().join("versions");
            std::fs::create_dir_all(&versions).unwrap();
            std::os::unix::fs::symlink(outside.path(), versions.join("new")).unwrap();
            assert!(
                publish_prepared_build(
                    payload.path(),
                    "new",
                    PreparedBuildChannel::Stable,
                    &payload.path().join("launcher"),
                    false
                )
                .is_err()
            );
            assert!(!outside.path().join(binary_name()).exists());
        });
    }

    #[test]
    fn symlink_payload_fails_before_store_mutation() {
        with_temp_jcode_home(|| {
            let payload = tempfile::tempdir().unwrap();
            std::os::unix::fs::symlink("absent", payload.path().join(binary_name())).unwrap();
            assert!(
                publish_prepared_build(
                    payload.path(),
                    "new",
                    PreparedBuildChannel::Stable,
                    &payload.path().join("launcher"),
                    false
                )
                .is_err()
            );
            assert!(!builds_dir().unwrap().join("versions/new").exists());
        });
    }

    #[test]
    fn termux_launcher_quotes_paths_and_preserves_arguments() {
        with_temp_jcode_home(|| {
            let payload = tempfile::tempdir().unwrap();
            executable(
                &payload.path().join(binary_name()),
                "#!/bin/sh\nprintf '%s\\n' \"${LD_PRELOAD-unset}\" \"$@\"\n",
            );
            let launcher = payload.path().join("quoted ' launcher");
            publish_prepared_build(
                payload.path(),
                "new'quoted",
                PreparedBuildChannel::Stable,
                &launcher,
                true,
            )
            .unwrap();
            let output = Command::new(&launcher)
                .arg("argument with space")
                .output()
                .unwrap();
            assert!(output.status.success());
            assert_eq!(
                String::from_utf8(output.stdout).unwrap(),
                "unset\nargument with space\n"
            );
        });
    }
}
