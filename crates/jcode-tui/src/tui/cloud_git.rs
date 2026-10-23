//! Independent, read-only git snapshots for a local TUI attached over SSH.
//! No render-time process or network I/O. Each source owns one bounded worker
//! and retries after failures. SSH does not require a matching remote Jcode.
use super::info_widget::{DirtyFile, GitInfo};
#[cfg(not(test))]
use std::path::PathBuf;
#[cfg(not(test))]
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tokio::io::AsyncReadExt;

#[derive(Clone, Debug, Default, PartialEq)]
pub enum RepoState {
    #[default]
    Loading,
    Unavailable,
    NoRepository,
    Ready(GitInfo),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DualGitInfo {
    pub local_is_source: bool,
    pub local: RepoState,
    pub cloud: RepoState,
}

// Fixed script and NUL-framed status avoid shell interpolation and ambiguous
// filenames. A failed status query must never turn into a clean snapshot.
const SCRIPT: &str = r#"
export GIT_OPTIONAL_LOCKS=0 LC_ALL=C
command -v git >/dev/null 2>&1 || exit 43
[ -d "$1" ] || exit 43
cd -- "$1" || exit 43
root=$(git rev-parse --show-toplevel 2>&1) || {
    case "$root" in *'not a git repository'*) exit 42 ;; *) exit 43 ;; esac
}
branch=$(git symbolic-ref --quiet --short HEAD 2>/dev/null || git rev-parse --short HEAD 2>/dev/null) || exit 43
counts=$(git rev-list --left-right --count 'HEAD...@{upstream}' 2>/dev/null) || counts='0 0'
printf 'jcode-git-v1\000%s\000%s\000%s\000' "$root" "$branch" "$counts"
git status --porcelain=v1 -z --untracked-files=all || exit 43
printf '\000'
git diff --numstat HEAD 2>/dev/null || :
"#;

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn command(
    host: Option<&str>,
    config: Option<&std::path::Path>,
    dir: &str,
) -> tokio::process::Command {
    script_command(host, config, dir, SCRIPT)
}

pub(crate) fn script_command(
    host: Option<&str>,
    config: Option<&std::path::Path>,
    dir: &str,
    script: &str,
) -> tokio::process::Command {
    let mut cmd = if let Some(host) = host {
        let mut cmd = tokio::process::Command::new("ssh");
        if let Some(config) = config {
            cmd.arg("-F").arg(config);
        }
        cmd.args([
            "-T",
            "-o",
            "BatchMode=yes",
            "-o",
            "StrictHostKeyChecking=yes",
            "-o",
            "ForwardAgent=no",
            "-o",
            "ForwardX11=no",
            "-o",
            "Tunnel=no",
            "-o",
            "AddKeysToAgent=no",
            "-o",
            "UpdateHostKeys=no",
            "-o",
            "ClearAllForwardings=yes",
            "-o",
            "PermitLocalCommand=no",
            "-o",
            "RemoteCommand=none",
            "-o",
            "SessionType=default",
            "-o",
            "ControlMaster=no",
            "-S",
            "none",
            "-o",
            "ConnectTimeout=8",
            "-o",
            "ServerAliveInterval=5",
            "-o",
            "ServerAliveCountMax=1",
        ]);
        cmd.arg("--")
            .arg(host)
            .arg(format!("sh -c {} sh {}", quote(script), quote(dir)));
        cmd
    } else {
        let mut cmd = tokio::process::Command::new("sh");
        cmd.args(["-c", script, "sh", dir]);
        cmd
    };
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    cmd
}

async fn probe(command: tokio::process::Command) -> RepoState {
    probe_with_timeout(command, Duration::from_secs(12)).await
}

async fn probe_with_timeout(command: tokio::process::Command, deadline: Duration) -> RepoState {
    match bounded_output(command, deadline, 1024 * 1024).await {
        Some((status, _)) if status.code() == Some(42) => RepoState::NoRepository,
        Some((status, bytes)) if status.success() => parse(&bytes)
            .map(RepoState::Ready)
            .unwrap_or(RepoState::Unavailable),
        _ => RepoState::Unavailable,
    }
}

pub(crate) async fn bounded_output(
    mut command: tokio::process::Command,
    deadline: Duration,
    limit: usize,
) -> Option<(std::process::ExitStatus, Vec<u8>)> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.as_std_mut().process_group(0);
        #[cfg(target_os = "linux")]
        {
            let parent = std::process::id();
            // Kill SSH and git even when the UI exits without running Drop.
            unsafe {
                command.as_std_mut().pre_exec(move || {
                    if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    if libc::getppid() as u32 != parent {
                        libc::raise(libc::SIGKILL);
                    }
                    Ok(())
                });
            }
        }
    }
    let Ok(mut child) = command.spawn() else {
        return None;
    };
    let stdout = child.stdout.take()?;
    let result = tokio::time::timeout(deadline, async {
        let mut bytes = Vec::new();
        stdout
            .take((limit + 1) as u64)
            .read_to_end(&mut bytes)
            .await
            .ok()?;
        if bytes.len() > limit {
            return None;
        }
        let status = child.wait().await.ok()?;
        Some((status, bytes))
    })
    .await;
    match result {
        Ok(Some(output)) => Some(output),
        _ => {
            #[cfg(unix)]
            if let Some(pid) = child.id() {
                // Own process group only, including ProxyCommand descendants.
                unsafe {
                    libc::kill(-(pid as i32), libc::SIGKILL);
                }
            }
            let _ = child.kill().await;
            let _ = child.wait().await;
            None
        }
    }
}

fn parse(bytes: &[u8]) -> Option<GitInfo> {
    let mut fields = bytes.splitn(5, |b| *b == 0);
    if fields.next()? != b"jcode-git-v1" {
        return None;
    }
    let root = String::from_utf8_lossy(fields.next()?).into_owned();
    let branch = String::from_utf8_lossy(fields.next()?).into_owned();
    let counts = String::from_utf8_lossy(fields.next()?);
    let mut counts = counts.split_whitespace();
    let mut info = GitInfo {
        repo_root: Some(root.into()),
        branch,
        ahead: counts.next()?.parse().ok()?,
        behind: counts.next()?.parse().ok()?,
        ..Default::default()
    };
    let mut records = fields.next()?.split(|b| *b == 0);
    loop {
        let record = records.next()?;
        if record.is_empty() {
            break;
        }
        if record.len() < 4 || record[2] != b' ' {
            return None;
        }
        let x = record[0];
        let y = record[1];
        if x == b'?' {
            info.untracked += 1;
        } else {
            if x != b' ' {
                info.staged += 1;
            }
            if y != b' ' {
                info.modified += 1;
            }
        }
        let path = String::from_utf8_lossy(&record[3..]).into_owned();
        if x == b'R' || x == b'C' || y == b'R' || y == b'C' {
            records.next()?;
        }
        info.dirty_total += 1;
        if info.dirty_files.len() < 10 {
            info.dirty_files.push(DirtyFile::new(
                super::app::helpers::porcelain_status_letter(x, y),
                path,
            ));
        }
    }
    let numstat = records.collect::<Vec<_>>().join(&0);
    let text = String::from_utf8_lossy(&numstat);
    let stats = super::app::helpers::parse_numstat(&text);
    for (added, removed) in stats.values() {
        info.added_total += added.unwrap_or(0);
        info.removed_total += removed.unwrap_or(0);
    }
    for file in &mut info.dirty_files {
        if let Some(&(added, removed)) = stats.get(&file.path) {
            file.added = added;
            file.removed = removed;
        }
    }
    Some(info)
}

/// Starts exactly two independent workers for this attach. No shared cache
/// with the ordinary local status-line probe, and no stale success on failure.
#[cfg(not(test))]
pub(crate) fn gather() -> Option<DualGitInfo> {
    let host = super::ssh_remote_host()?;
    if host.starts_with('-') || host.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    static CACHE: OnceLock<Arc<Mutex<DualGitInfo>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| {
        let cache = Arc::new(Mutex::new(DualGitInfo {
            local_is_source: std::env::var("JCODE_SSH_LOCAL_IS_SOURCE").as_deref() == Ok("1"),
            ..Default::default()
        }));
        let local = std::env::var("JCODE_SSH_LOCAL_WORKING_DIR").ok();
        let remote = std::env::var("JCODE_SSH_WORKING_DIR").ok();
        let config = std::env::var_os("JCODE_SSH_CONFIG").map(PathBuf::from);
        for (is_cloud, dir) in [(false, local), (true, remote)] {
            let cache = cache.clone();
            let host = host.clone();
            let config = config.clone();
            std::thread::spawn(move || {
                let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                else {
                    return;
                };
                runtime.block_on(async move {
                    loop {
                        let state = if let Some(dir) = &dir {
                            probe(command(
                                is_cloud.then_some(host.as_str()),
                                config.as_deref(),
                                dir,
                            ))
                            .await
                        } else {
                            RepoState::Unavailable
                        };
                        let changed = {
                            let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());
                            let slot = if is_cloud {
                                &mut guard.cloud
                            } else {
                                &mut guard.local
                            };
                            let changed = *slot != state;
                            *slot = state;
                            changed
                        };
                        if changed {
                            crate::bus::Bus::global()
                                .publish(crate::bus::BusEvent::GitInfoRefreshed);
                        }
                        tokio::time::sleep(Duration::from_secs(5)).await;
                    }
                });
            });
        }
        cache
    });
    Some(cache.lock().unwrap_or_else(|e| e.into_inner()).clone())
}

#[cfg(test)]
pub(crate) fn gather() -> Option<DualGitInfo> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn snapshot_handles_rename_newline_and_independent_counts() {
        let info = parse(b"jcode-git-v1\0/repo\0main\02 3\0R  new name\0old name\0?? a\nb\0 M tracked\0\01\t2\ttracked\n").unwrap();
        assert_eq!(
            (info.staged, info.modified, info.untracked, info.dirty_total),
            (1, 1, 1, 3)
        );
        assert_eq!(info.dirty_files[1].path, "a\nb");
        assert_eq!(
            (
                info.ahead,
                info.behind,
                info.added_total,
                info.removed_total
            ),
            (2, 3, 1, 2)
        );
        assert!(parse(b"bad response").is_none());
        assert!(parse(b"jcode-git-v1\0/repo\0main\00 0\0 M truncated").is_none());
    }
    #[tokio::test]
    async fn live_independent_repositories_and_missing_path() {
        let local = tempfile::tempdir().unwrap();
        let cloud = tempfile::tempdir().unwrap();
        for dir in [local.path(), cloud.path()] {
            assert!(
                std::process::Command::new("git")
                    .args(["init", "-q"])
                    .arg(dir)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        std::fs::write(local.path().join("local-only"), "local\n").unwrap();
        std::fs::write(cloud.path().join("cloud-only"), "remote\n").unwrap();
        let RepoState::Ready(local_info) =
            probe(command(None, None, local.path().to_str().unwrap())).await
        else {
            panic!("local probe")
        };
        let RepoState::Ready(cloud_info) =
            probe(command(None, None, cloud.path().to_str().unwrap())).await
        else {
            panic!("cloud probe")
        };
        assert_eq!(local_info.dirty_files[0].path, "local-only");
        assert_eq!(cloud_info.dirty_files[0].path, "cloud-only");
        std::fs::remove_file(cloud.path().join("cloud-only")).unwrap();
        let RepoState::Ready(clean) =
            probe(command(None, None, cloud.path().to_str().unwrap())).await
        else {
            panic!("clean probe")
        };
        assert_eq!(clean.dirty_total, 0);
        assert_eq!(
            probe(command(None, None, "/nonexistent/jcode-git-probe")).await,
            RepoState::Unavailable
        );
        let no_repo = tempfile::tempdir().unwrap();
        assert_eq!(
            probe(command(None, None, no_repo.path().to_str().unwrap())).await,
            RepoState::NoRepository
        );
    }
    #[tokio::test]
    async fn failed_or_oversized_probes_never_look_clean_and_timeout_is_bounded() {
        let mut failed = tokio::process::Command::new("sh");
        failed
            .args([
                "-c",
                "printf 'jcode-git-v1\\000/repo\\000main\\0000 0\\000\\000'; exit 1",
            ])
            .stdout(std::process::Stdio::piped());
        assert_eq!(probe(failed).await, RepoState::Unavailable);
        let mut oversized = tokio::process::Command::new("sh");
        oversized
            .args(["-c", "head -c 1048577 /dev/zero"])
            .stdout(std::process::Stdio::piped());
        assert_eq!(probe(oversized).await, RepoState::Unavailable);
        let mut stalled = tokio::process::Command::new("sh");
        stalled
            .args(["-c", "sleep 30"])
            .stdout(std::process::Stdio::piped());
        let start = std::time::Instant::now();
        assert_eq!(
            probe_with_timeout(stalled, Duration::from_millis(40)).await,
            RepoState::Unavailable
        );
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn ssh_command_quotes_workspace_and_reuses_private_config() {
        let cmd = command(
            Some("cloud-host"),
            Some(std::path::Path::new("/private/config")),
            "/repo/a'b",
        );
        let args: Vec<_> = cmd
            .as_std()
            .get_args()
            .map(|s| s.to_string_lossy().to_string())
            .collect();
        assert_eq!(&args[..2], ["-F", "/private/config"]);
        assert!(args.last().unwrap().ends_with("'/repo/a'\\''b'"));
        assert!(args.iter().any(|s| s == "BatchMode=yes"));
        assert!(args.iter().any(|s| s == "ForwardX11=no"));
        assert!(args.iter().any(|s| s == "Tunnel=no"));
        assert!(args.iter().any(|s| s == "AddKeysToAgent=no"));
        assert!(args.iter().any(|s| s == "UpdateHostKeys=no"));
    }
}
