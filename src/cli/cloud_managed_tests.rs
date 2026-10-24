//! Local-only execution of the exact generated shell, with downloaded artifacts
//! supplied by a fake curl. No network, cloud host or user installation is used.
use super::*;

#[test]
fn release_policy_is_exact_and_development_has_no_fallback() {
    assert_eq!(
        release_target(true, "0.99.123", None).unwrap().as_deref(),
        Some("v0.99.123")
    );
    assert_eq!(release_target(false, "0.99.123-dev", None).unwrap(), None);
    assert_eq!(
        release_target(false, "dev", Some("v1.2.3"))
            .unwrap()
            .as_deref(),
        Some("v1.2.3")
    );
    assert_eq!(
        release_target(true, "1.2.3", Some("2.3.4"))
            .unwrap()
            .as_deref(),
        Some("v2.3.4")
    );
    for bad in [
        "",
        "latest",
        "v1.2",
        "v1.2.3-dev",
        "v1.2.3/evil",
        "v1.2.3';id",
        " v1.2.3",
        "v01.2.3",
    ] {
        assert!(release_target(true, "1.2.3", Some(bad)).is_err(), "{bad}");
    }
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod shell {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    struct Fixture {
        dir: tempfile::TempDir,
    }

    fn executable(path: &Path, text: &str) {
        std::fs::write(path, text).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn binary(version: &str, credentials: bool, fail_after_switch: bool) -> String {
        format!(
            r#"#!/bin/sh
set -eu
[ -z "${{JCODE_RUNTIME_RELEASE_SEMVER:-}}" ] || exit 97
if [ {fail_after_switch} = true ] && [ "$0" = "$HOME/opt/jcode" ]; then exit 96; fi
case "$*" in
  '--no-update --no-selfdev --version') echo 'jcode {version} (test)' ;;
  '--no-update --no-selfdev cloud receive --help') exit 0 ;;
  '--no-update --no-selfdev cloud cancel-return --help') exit 0 ;;
  '--no-update --no-selfdev cloud credentials --help') {credentials} ;;
  '--no-update --no-selfdev cloud router-credentials --help') {credentials} ;;
  *) exit 95 ;;
esac
"#
        )
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            for folder in [".local/bin", "fixtures", "mocks", "payload", "opt"] {
                std::fs::create_dir_all(dir.path().join(folder)).unwrap();
            }
            executable(
                &dir.path().join("mocks/curl"),
                r#"#!/bin/sh
set -eu
url=
while [ "$#" -gt 0 ]; do
  case "$1" in
    https://*) url=$1 ;;
    -o) shift; dest=$1 ;;
  esac
  shift
done
printf '%s\n' "$url" >> "$HOME/downloads"
cp "$HOME/fixtures/${url##*/}" "$dest"
"#,
            );
            // All possible privilege/service operations must fail the test.
            for name in ["sudo", "systemctl", "kill", "pkill"] {
                executable(
                    &dir.path().join("mocks").join(name),
                    "#!/bin/sh\necho forbidden >&2\nexit 99\n",
                );
            }
            Self { dir }
        }

        fn cli(&self) -> PathBuf {
            self.dir.path().join(".local/bin/jcode")
        }

        fn backend(&self) -> PathBuf {
            self.dir.path().join("opt/jcode")
        }

        fn installed(&self, credentials: bool) {
            executable(&self.backend(), &binary("v0.99.1", credentials, false));
            executable(
                &self.cli(),
                "#!/bin/sh\nexport JCODE_SERVER_NAME=preserved-host-config\nexec \"$HOME/opt/jcode\" \"$@\"\n",
            );
        }

        fn artifact(&self, credentials: bool, fail_after_switch: bool, extra: Option<&str>) {
            let payload = self.dir.path().join("payload");
            // Same symlink-resolving wrapper layout as build_linux_compat.sh.
            executable(
                &payload.join("jcode-linux-x86_64"),
                "#!/bin/sh\np=$(readlink -f \"$0\")\nexec \"$p.bin\" \"$@\"\n",
            );
            // For post-switch failure injection, use a direct wrapper because the
            // production wrapper resolves its symlink before launching .bin.
            if fail_after_switch {
                executable(
                    &payload.join("jcode-linux-x86_64"),
                    &binary("v0.99.123", credentials, true),
                );
            }
            executable(
                &payload.join("jcode-linux-x86_64.bin"),
                &binary("v0.99.123", credentials, false),
            );
            std::fs::write(payload.join("libssl.so.10"), "fixture library").unwrap();
            let asset = self.dir.path().join("fixtures/jcode-linux-x86_64.tar.gz");
            let mut tar = Command::new("tar");
            tar.arg("czf").arg(&asset).arg("-C").arg(&payload).args([
                "jcode-linux-x86_64",
                "jcode-linux-x86_64.bin",
                "libssl.so.10",
            ]);
            if let Some(extra) = extra {
                std::fs::write(payload.join(extra), "unexpected").unwrap();
                tar.arg(extra);
            }
            assert!(tar.status().unwrap().success());
            self.checksum();
        }

        fn checksum(&self) {
            let output = Command::new("sha256sum")
                .current_dir(self.dir.path().join("fixtures"))
                .arg("jcode-linux-x86_64.tar.gz")
                .output()
                .unwrap();
            assert!(output.status.success());
            std::fs::write(self.dir.path().join("fixtures/SHA256SUMS"), output.stdout).unwrap();
        }

        fn run(&self, target: Option<&str>) -> std::process::Output {
            let script =
                prepare_script_for_paths("", target, self.backend().to_str().unwrap(), false);
            let path = self.dir.path().join("prepare.sh");
            std::fs::write(&path, script).unwrap();
            Command::new("sh")
                .arg(path)
                .env("HOME", self.dir.path())
                .env(
                    "PATH",
                    format!("{}:/usr/bin:/bin", self.dir.path().join("mocks").display()),
                )
                .env("JCODE_RUNTIME_RELEASE_SEMVER", "9.9.9")
                .env_remove("JCODE_CLOUD_FORCE_UPGRADE")
                .output()
                .unwrap()
        }

        fn assert_old(&self) {
            assert!(!self.backend().is_symlink());
            assert!(
                std::fs::read_to_string(self.backend())
                    .unwrap()
                    .contains("v0.99.1")
            );
            assert!(
                std::fs::read_to_string(self.cli())
                    .unwrap()
                    .contains("preserved-host-config")
            );
            assert!(
                !self
                    .dir
                    .path()
                    .join(".local/share/jcode/cloud-releases/.prepare-lock")
                    .exists()
            );
        }
    }

    #[test]
    fn development_keeps_capable_cli_without_downloading() {
        let f = Fixture::new();
        f.installed(true);
        let result = f.run(None);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        f.assert_old();
        assert!(!f.dir.path().join("downloads").exists());
    }

    #[test]
    fn development_rejects_receive_only_cli_without_downloading() {
        let f = Fixture::new();
        f.installed(false);
        let result = f.run(None);
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains("JCODE_CLOUD_RELEASE"));
        f.assert_old();
        assert!(!f.dir.path().join("downloads").exists());
    }

    #[test]
    fn published_release_upgrades_effective_cli_and_preserves_previous() {
        let f = Fixture::new();
        f.installed(false);
        f.artifact(true, false, None);
        let result = f.run(Some("v0.99.123"));
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let target = std::fs::read_link(f.backend()).unwrap();
        assert!(target.is_file());
        assert!(
            std::fs::read_to_string(f.cli())
                .unwrap()
                .contains("preserved-host-config")
        );
        let backup = std::fs::read_dir(f.backend().parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("jcode.previous.")
            })
            .unwrap();
        assert!(std::fs::read_to_string(backup).unwrap().contains("v0.99.1"));
        assert!(target.with_file_name("libssl.so.10").exists());
        let downloads = std::fs::read_to_string(f.dir.path().join("downloads")).unwrap();
        assert!(downloads.contains("/v0.99.123/jcode-linux-x86_64.tar.gz"));
        // Exact version plus both capabilities makes subsequent runs idempotent.
        let result = f.run(Some("v0.99.123"));
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(
            std::fs::read_to_string(f.dir.path().join("downloads")).unwrap(),
            downloads
        );
    }

    #[test]
    fn published_policy_keeps_newer_capable_stable_releases() {
        for (version, upgrade) in [
            ("v0.99.123", false),
            ("v0.99.999", false),
            ("v0.100.0", false),
            ("v1.0.0", false),
            ("v0.99.99", true),
        ] {
            let f = Fixture::new();
            f.installed(true);
            executable(&f.backend(), &binary(version, true, false));
            f.artifact(true, false, None);
            let result = f.run(Some("v0.99.123"));
            assert!(
                result.status.success(),
                "{version}: {}",
                String::from_utf8_lossy(&result.stderr)
            );
            assert_eq!(f.backend().is_symlink(), upgrade, "{version}");
            assert_eq!(
                f.dir.path().join("downloads").exists(),
                upgrade,
                "{version}"
            );
        }
    }

    #[test]
    fn rejects_missing_credentials_wrong_checksum_and_unexpected_members() {
        for mode in [
            "credentials",
            "checksum",
            "duplicate-checksum",
            "members",
            "version",
            "symlink",
        ] {
            let f = Fixture::new();
            f.installed(true);
            f.artifact(
                mode != "credentials",
                false,
                (mode == "members").then_some("evil"),
            );
            if mode == "checksum" {
                std::fs::write(
                    f.dir.path().join("fixtures/SHA256SUMS"),
                    format!("{}  jcode-linux-x86_64.tar.gz\n", "0".repeat(64)),
                )
                .unwrap();
            }
            if mode == "duplicate-checksum" {
                let sums = f.dir.path().join("fixtures/SHA256SUMS");
                let original = std::fs::read_to_string(&sums).unwrap();
                std::fs::write(sums, original.repeat(2)).unwrap();
            }
            if mode == "symlink" {
                let payload = f.dir.path().join("payload");
                std::fs::remove_file(payload.join("libssl.so.10")).unwrap();
                std::os::unix::fs::symlink("/etc/passwd", payload.join("libssl.so.10")).unwrap();
                assert!(
                    Command::new("tar")
                        .arg("czf")
                        .arg(f.dir.path().join("fixtures/jcode-linux-x86_64.tar.gz"))
                        .arg("-C")
                        .arg(payload)
                        .args([
                            "jcode-linux-x86_64",
                            "jcode-linux-x86_64.bin",
                            "libssl.so.10"
                        ])
                        .status()
                        .unwrap()
                        .success()
                );
                f.checksum();
            }
            let result = f.run(Some(if mode == "version" {
                "v0.99.124"
            } else {
                "v0.99.123"
            }));
            assert!(!result.status.success(), "mode {mode}");
            f.assert_old();
        }
    }

    #[test]
    fn failed_post_switch_verification_restores_previous_binary() {
        let f = Fixture::new();
        f.installed(true);
        f.artifact(true, true, None);
        let result = f.run(Some("v0.99.123"));
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains("restored previous CLI"));
        f.assert_old();
    }

    #[test]
    fn failed_effective_cli_verification_restores_relative_backend_symlink() {
        let f = Fixture::new();
        f.installed(true);
        let original = f.backend().with_file_name("original");
        std::fs::rename(f.backend(), &original).unwrap();
        std::os::unix::fs::symlink("original", f.backend()).unwrap();
        // A launcher no longer delegating to the managed backend must not be
        // silently replaced or leave the backend upgraded after verification.
        executable(&f.cli(), &binary("v0.99.1", true, false));
        f.artifact(true, false, None);
        let result = f.run(Some("v0.99.123"));
        assert!(!result.status.success());
        assert_eq!(
            std::fs::read_link(f.backend()).unwrap(),
            Path::new("original")
        );
        assert!(
            std::fs::read_to_string(original)
                .unwrap()
                .contains("v0.99.1")
        );
        assert!(
            std::fs::read_to_string(f.cli())
                .unwrap()
                .contains("v0.99.1")
        );
        assert!(String::from_utf8_lossy(&result.stderr).contains("restored previous CLI"));
    }

    #[test]
    fn rollback_failure_retains_recovery_files_without_claiming_success() {
        let f = Fixture::new();
        f.installed(true);
        f.artifact(true, true, None);
        executable(
            &f.dir.path().join("mocks/mv"),
            r#"#!/bin/sh
case "$*" in *jcode.previous.*) exit 98 ;; esac
exec /usr/bin/mv "$@"
"#,
        );
        let result = f.run(Some("v0.99.123"));
        assert!(!result.status.success());
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(stderr.contains("rollback failed"));
        assert!(!stderr.contains("restored previous CLI"));
        let candidate = std::fs::read_link(f.backend()).unwrap();
        assert!(candidate.is_file());
        assert!(
            std::fs::read_dir(f.backend().parent().unwrap())
                .unwrap()
                .any(|entry| entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with("jcode.previous."))
        );
    }

    #[test]
    fn home_shell_metacharacters_are_quoted_without_loss() {
        let value = "/home/a'b $(touch should-not-exist)";
        let result = Command::new("sh")
            .arg("-c")
            .arg(format!("printf %s {}", shell_quote(value)))
            .output()
            .unwrap();
        assert!(result.status.success());
        assert_eq!(String::from_utf8(result.stdout).unwrap(), value);
    }
}
