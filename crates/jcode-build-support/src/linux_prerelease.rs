use super::SourceState;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinuxReleaseStatus {
    pub sha: String,
    #[serde(default)]
    pub tag: Option<String>,
    pub state: String,
    pub step: String,
    #[serde(default)]
    pub macos_status: Option<String>,
    #[serde(default)]
    pub discord_status: Option<String>,
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub log_file: Option<String>,
}

pub fn linux_release_status_path() -> Option<PathBuf> {
    jcode_storage::jcode_dir()
        .ok()
        .map(|dir| dir.join("logs").join("linux-release-status.json"))
}

pub fn read_linux_release_status() -> Option<LinuxReleaseStatus> {
    let path = linux_release_status_path()?;
    read_linux_release_status_from(&path)
}

pub fn read_linux_release_status_from(path: &Path) -> Option<LinuxReleaseStatus> {
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Snapshot eligibility before compiling. A feature branch pointing to master is not master.
pub fn linux_prerelease_candidate(repo: &Path, source: &SourceState) -> bool {
    let matches = |args: &[&str], expected: &str| {
        Command::new("git")
            .current_dir(repo)
            .args(args)
            .output()
            .is_ok_and(|out| {
                out.status.success() && String::from_utf8_lossy(&out.stdout).trim_end() == expected
            })
    };
    !source.dirty
        && source.full_hash.len() == 40
        && source.full_hash.bytes().all(|b| b.is_ascii_hexdigit())
        && matches(&["config", "--bool", "jcode.linuxPrerelease"], "true")
        && matches(&["symbolic-ref", "-q", "HEAD"], "refs/heads/master")
        && matches(&["rev-parse", "HEAD"], &source.full_hash)
        && matches(
            &[
                "status",
                "--porcelain=v1",
                "--untracked-files=all",
                "--ignore-submodules=none",
            ],
            "",
        )
}

const DISPATCH_SCRIPT: &str = include_str!("linux_prerelease.sh");

/// Network I/O happens in a child process so build/reload can finish independently.
pub fn dispatch_linux_prerelease(repo: &Path, source: &SourceState, release_notes: Option<&str>) {
    let child = Command::new("sh")
        .current_dir(repo)
        .args(["-c", DISPATCH_SCRIPT, "linux-prerelease", &source.full_hash])
        .args(release_notes)
        .env("GH_HOST", "github.com")
        .env("GH_PROMPT_DISABLED", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let mut child = match child {
        Ok(child) => child,
        Err(error) => {
            eprintln!("Could not launch Linux prerelease dispatch: {error}");
            return;
        }
    };
    if let Err(error) = std::thread::Builder::new()
        .name("linux-prerelease".into())
        .spawn(move || child.wait())
    {
        eprintln!("Could not start Linux prerelease process watcher: {error}");
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    #[test]
    fn prerelease_checks_local_and_remote_state_before_dispatch() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        fs::create_dir(&repo).unwrap();
        let git = |args: &[&str]| {
            assert!(
                Command::new("git")
                    .current_dir(&repo)
                    .args(args)
                    .output()
                    .unwrap()
                    .status
                    .success()
            )
        };
        git(&["init", "-b", "master"]);
        git(&[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.org",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--allow-empty",
            "-m",
            "test",
        ]);
        let source = crate::current_source_state(&repo).unwrap();
        git(&["config", "jcode.linuxPrerelease", "false"]);
        assert!(!linux_prerelease_candidate(&repo, &source));
        git(&["config", "jcode.linuxPrerelease", "true"]);
        assert!(linux_prerelease_candidate(&repo, &source));
        let mock = dir.path().join("gh");
        fs::write(
            &mock,
            "#!/bin/sh\nif [ \"$1\" = api ]; then echo \"$TIP\"; else printf '%s\\n' \"$@\"; fi\n",
        )
        .unwrap();
        fs::set_permissions(mock, fs::Permissions::from_mode(0o755)).unwrap();
        let path = std::env::join_paths(
            std::iter::once(dir.path().to_owned())
                .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
        )
        .unwrap();
        let dispatch = |tip: &str, notes: Option<&str>| {
            let out = Command::new("sh")
                .current_dir(&repo)
                .env("PATH", &path)
                .env("TIP", tip)
                .args(["-c", DISPATCH_SCRIPT, "hook", &source.full_hash])
                .args(notes)
                .output()
                .unwrap();
            assert!(out.status.success());
            String::from_utf8(out.stdout).unwrap()
        };
        assert!(dispatch("not-pushed", None).is_empty());
        assert_eq!(
            dispatch(&source.full_hash, None),
            format!(
                "workflow\nrun\nselfdev-linux.yml\n--repo\n1jehuang/jcode\n--ref\nmaster\n-f\nsha={}\n",
                source.full_hash
            )
        );
        let notes = "First line with spaces
$(touch injected) * --ref other";
        assert_eq!(
            dispatch(&source.full_hash, Some(notes)),
            format!(
                "{}-f\nrelease_notes={notes}\n",
                dispatch(&source.full_hash, None)
            )
        );
        assert!(!repo.join("injected").exists());
        fs::write(repo.join("dirty"), "private").unwrap();
        assert!(!linux_prerelease_candidate(&repo, &source));
        assert!(dispatch(&source.full_hash, None).is_empty());
        fs::remove_file(repo.join("dirty")).unwrap();
        git(&["checkout", "-b", "other"]);
        assert!(!linux_prerelease_candidate(&repo, &source));
        assert!(dispatch(&source.full_hash, None).is_empty());
    }
}
