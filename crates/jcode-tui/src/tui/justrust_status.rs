//! Nonblocking consumer of justrust's public `status --json` interface.
//! This is UI-only telemetry: missing justrust or a failed query never affects
//! a tool invocation, and an SSH client must not show the laptop's builds.

use serde::Deserialize;
use std::path::Path;
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;
use tokio::sync::oneshot;

const POLL_INTERVAL: Duration = Duration::from_secs(1);
const RETRY_INTERVAL: Duration = Duration::from_secs(10);
const QUERY_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_RESPONSE: usize = 512 * 1024;
const COMPLETION_RETENTION: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, Default, Deserialize)]
pub(crate) struct Snapshot {
    #[serde(default)]
    active: Vec<Active>,
    recent: Option<Finished>,
}

#[derive(Debug, Default, Deserialize)]
struct Active {
    #[serde(default)]
    cwd: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    label: String,
    #[serde(default)]
    place: String,
    #[serde(default)]
    elapsed: f64,
    agent_session: Option<String>,
    #[serde(default)]
    ncpu: usize,
    cpu_cores: Option<f64>,
    #[serde(default)]
    live: Live,
    estimate: Option<Estimate>,
}

#[derive(Debug, Default, Deserialize)]
struct Live {
    #[serde(default)]
    phase: String,
    #[serde(default)]
    done: usize,
    #[serde(default)]
    total: usize,
    #[serde(default)]
    active: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Estimate {
    remaining: f64,
}

#[derive(Debug, Clone, Deserialize)]
struct Finished {
    #[serde(default)]
    cwd: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    label: String,
    #[serde(default)]
    place: String,
    wall: f64,
    ok: bool,
    ago: f64,
    agent_session: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Active,
    Success,
    Failure,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusRow {
    pub text: String,
    pub tone: Tone,
}

fn clean(value: &str) -> String {
    value
        .chars()
        .filter(|c| !c.is_control())
        .take(160)
        .collect()
}

fn duration(seconds: f64) -> String {
    let seconds = if seconds.is_finite() {
        seconds.max(0.0)
    } else {
        0.0
    };
    if seconds < 60.0 {
        format!("{seconds:.0}s")
    } else if seconds < 3600.0 {
        format!("{}m{:02}s", seconds as u64 / 60, seconds as u64 % 60)
    } else {
        format!("{}h{:02}m", seconds as u64 / 3600, seconds as u64 / 60 % 60)
    }
}

fn location(place: &str) -> &'static str {
    match place {
        "hosted" => "remote/hosted",
        "remote" => "remote",
        "local" => "local",
        _ => "location unknown",
    }
}

fn context(cwd: &str, args: &[String], label: &str) -> String {
    let repo = Path::new(cwd)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let operation = args.iter().find(|arg| {
        matches!(
            arg.as_str(),
            "build" | "b" | "check" | "c" | "test" | "t" | "clippy" | "run" | "r" | "bench" | "doc"
        )
    });
    match operation {
        Some(operation) => format!("{} {}", clean(operation), clean(repo)),
        None => clean(label),
    }
}

fn relevance(
    run_session: Option<&str>,
    cwd: &str,
    session: Option<&str>,
    repo: Option<&str>,
) -> u8 {
    if session.is_some() && run_session == session {
        2
    } else if repo
        .is_some_and(|repo| !repo.is_empty() && !cwd.is_empty() && Path::new(cwd).starts_with(repo))
    {
        1
    } else {
        0
    }
}

impl Snapshot {
    fn row(&self, session: Option<&str>, repo: Option<&str>, age: Duration) -> Option<StatusRow> {
        let matching = self
            .active
            .iter()
            .filter(|run| relevance(run.agent_session.as_deref(), &run.cwd, session, repo) > 0);
        let matching_count = matching.clone().count();
        let active = matching
            .max_by_key(|run| relevance(run.agent_session.as_deref(), &run.cwd, session, repo));
        if let Some(run) = active {
            let mut text = format!(
                "justrust [{}] {} · {}",
                location(&run.place),
                context(&run.cwd, &run.args, &run.label),
                duration(run.elapsed + age.as_secs_f64())
            );
            match run
                .cpu_cores
                .filter(|cores| cores.is_finite() && *cores >= 0.0)
            {
                Some(cores) if run.ncpu > 0 => {
                    text.push_str(&format!(" · {cores:.1}/{} cores", run.ncpu))
                }
                Some(cores) => text.push_str(&format!(" · {cores:.1} cores")),
                None if run.ncpu > 0 => text.push_str(&format!(" · ?/{} cores", run.ncpu)),
                None => {}
            }
            let phase = clean(&run.live.phase);
            if !phase.is_empty() {
                text.push_str(&format!(" · {phase}"));
            }
            if run.live.total > 0 {
                text.push_str(&format!(" {}/{}", run.live.done, run.live.total));
            }
            if let Some(unit) = run.live.active.first() {
                text.push_str(&format!(" · {}", clean(unit)));
            }
            if let Some(estimate) = &run.estimate
                && estimate.remaining.is_finite()
                && estimate.remaining > 0.0
            {
                text.push_str(&format!(
                    " · ~{} left",
                    duration((estimate.remaining - age.as_secs_f64()).max(0.0))
                ));
            }
            if matching_count > 1 {
                text.push_str(&format!(" · +{} builds", matching_count - 1));
            }
            return Some(StatusRow {
                text,
                tone: Tone::Active,
            });
        }
        let run = self.recent.as_ref()?;
        if !run.ago.is_finite()
            || run.ago < 0.0
            || run.ago + age.as_secs_f64() > COMPLETION_RETENTION.as_secs_f64()
        {
            return None;
        }
        // Scope is required even with older producers: unknown ownership must
        // not expose another session's completion.
        if relevance(run.agent_session.as_deref(), &run.cwd, session, repo) == 0 {
            return None;
        }
        Some(StatusRow {
            text: format!(
                "justrust [{}] {} · {} in {}",
                location(&run.place),
                context(&run.cwd, &run.args, &run.label),
                if run.ok { "finished" } else { "failed" },
                duration(run.wall)
            ),
            tone: if run.ok { Tone::Success } else { Tone::Failure },
        })
    }
}

async fn query(program: &std::ffi::OsStr) -> Option<Snapshot> {
    query_with_timeout(program, QUERY_TIMEOUT).await
}

async fn query_with_timeout(program: &std::ffi::OsStr, timeout: Duration) -> Option<Snapshot> {
    tokio::time::timeout(timeout, async {
        let mut child = tokio::process::Command::new(program)
            .args(["status", "--json"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .ok()?;
        let mut bytes = Vec::new();
        child
            .stdout
            .take()?
            .take((MAX_RESPONSE + 1) as u64)
            .read_to_end(&mut bytes)
            .await
            .ok()?;
        if bytes.len() > MAX_RESPONSE {
            let _ = child.kill().await;
            return None;
        }
        if !child.wait().await.ok()?.success() {
            return None;
        }
        serde_json::from_slice(&bytes).ok()
    })
    .await
    .ok()
    .flatten()
}

#[derive(Default)]
pub(crate) struct Poller {
    pending: Option<oneshot::Receiver<Option<Snapshot>>>,
    snapshot: Option<Snapshot>,
    received: Option<Instant>,
    next_poll: Option<Instant>,
    row: Option<StatusRow>,
    last_finished: Option<(Finished, Instant)>,
}

impl Poller {
    pub(crate) fn row(&self) -> Option<&StatusRow> {
        self.row.as_ref()
    }

    /// Never waits on the subprocess or the filesystem on the UI thread.
    pub(crate) fn refresh(
        &mut self,
        session: Option<&str>,
        repo: Option<&str>,
        enabled: bool,
    ) -> bool {
        let previous = self.row.clone();
        if !enabled {
            self.row = None;
            self.snapshot = None;
            self.last_finished = None;
            self.pending = None;
            return self.row != previous;
        }
        let now = Instant::now();
        if let Some(receiver) = &mut self.pending {
            match receiver.try_recv() {
                Ok(snapshot) => {
                    let retry = if snapshot.is_some() {
                        POLL_INTERVAL
                    } else {
                        RETRY_INTERVAL
                    };
                    if let Some(run) = snapshot
                        .as_ref()
                        .and_then(|snapshot| snapshot.recent.as_ref())
                        && relevance(run.agent_session.as_deref(), &run.cwd, session, repo) > 0
                    {
                        self.last_finished = Some((run.clone(), now));
                    }
                    self.snapshot = snapshot;
                    self.received = Some(now);
                    self.pending = None;
                    self.next_poll = Some(now + retry);
                }
                Err(oneshot::error::TryRecvError::Closed) => {
                    self.pending = None;
                    self.snapshot = None;
                    self.next_poll = Some(now + RETRY_INTERVAL);
                }
                Err(oneshot::error::TryRecvError::Empty) => {}
            }
        }
        self.row = self.snapshot.as_ref().and_then(|snapshot| {
            let age = self
                .received
                .map(|at| now.saturating_duration_since(at))
                .unwrap_or_default();
            (age < Duration::from_secs(5))
                .then(|| snapshot.row(session, repo, age))
                .flatten()
        });
        if self.row.is_none()
            && let Some((run, received)) = &self.last_finished
        {
            self.row = Snapshot {
                active: Vec::new(),
                recent: Some(run.clone()),
            }
            .row(session, repo, now.saturating_duration_since(*received));
        }
        if self.pending.is_none()
            && self.next_poll.is_none_or(|next| now >= next)
            && let Ok(runtime) = tokio::runtime::Handle::try_current()
        {
            let (sender, receiver) = oneshot::channel();
            self.pending = Some(receiver);
            runtime.spawn(async move {
                let _ = sender.send(query(std::ffi::OsStr::new("justrust")).await);
            });
        }
        self.row != previous
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_waybar_api_remains_usable_without_resource_fields() {
        let snapshot: Snapshot = serde_json::from_str(r#"{"active":[{"cwd":"/repo/engine","args":["test","--lib"],"place":"hosted","elapsed":51,"live":{"phase":"testing"}}],"recent":null}"#).unwrap();
        let row = snapshot
            .row(None, Some("/repo/engine"), Duration::ZERO)
            .unwrap();
        assert_eq!(
            row.text,
            "justrust [remote/hosted] test engine · 51s · testing"
        );
        assert_eq!(row.tone, Tone::Active);
    }

    #[test]
    fn session_match_wins_over_repo_and_global_builds() {
        let snapshot: Snapshot = serde_json::from_str(r#"{"active":[{"cwd":"/repo/engine","args":["check"],"place":"local"},{"cwd":"/other/rewrite","args":["test"],"place":"hosted","agent_session":"ours","elapsed":100,"ncpu":96,"cpu_cores":7.8,"live":{"phase":"compiling","done":2,"total":10}},{"cwd":"/other/unrelated","args":["build"],"place":"local"}]}"#).unwrap();
        let row = snapshot
            .row(Some("ours"), Some("/repo/engine"), Duration::ZERO)
            .unwrap();
        assert!(
            row.text
                .contains("test rewrite · 1m40s · 7.8/96 cores · compiling 2/10")
        );
        assert!(row.text.ends_with("+1 builds"));
    }

    #[test]
    fn unrelated_builds_are_hidden_and_not_counted() {
        let snapshot: Snapshot = serde_json::from_str(
            r#"{"active":[{"cwd":"/other","agent_session":"theirs"},{"cwd":"/repo/app2"}]}"#,
        )
        .unwrap();
        assert!(
            snapshot
                .row(Some("ours"), Some("/repo/app"), Duration::ZERO)
                .is_none()
        );
        assert!(snapshot.row(None, None, Duration::ZERO).is_none());
        let scoped: Snapshot = serde_json::from_str(r#"{"active":[{"cwd":"/repo/app/subcrate","args":["check"],"agent_session":"theirs"},{"cwd":"/other"}]}"#).unwrap();
        let row = scoped
            .row(Some("ours"), Some("/repo/app"), Duration::ZERO)
            .unwrap();
        assert!(row.text.contains("check subcrate"));
        assert!(!row.text.contains("builds"));
    }

    #[test]
    fn unrelated_active_build_does_not_hide_scoped_completion() {
        let snapshot: Snapshot = serde_json::from_str(r#"{"active":[{"cwd":"/other"}],"recent":{"cwd":"/repo/app","wall":1,"ok":true,"ago":0}}"#).unwrap();
        assert_eq!(
            snapshot
                .row(Some("ours"), Some("/repo/app"), Duration::ZERO)
                .unwrap()
                .tone,
            Tone::Success
        );
        let unknown: Snapshot =
            serde_json::from_str(r#"{"recent":{"wall":1,"ok":true,"ago":0}}"#).unwrap();
        assert!(
            unknown
                .row(Some("ours"), Some("/repo/app"), Duration::ZERO)
                .is_none()
        );
    }

    #[test]
    fn repo_matching_respects_path_components() {
        assert_eq!(relevance(None, "/repo/app2", None, Some("/repo/app")), 0);
        assert_eq!(
            relevance(None, "/repo/app/crate", None, Some("/repo/app")),
            1
        );
    }

    #[test]
    fn stale_and_other_session_completions_are_hidden() {
        let snapshot: Snapshot = serde_json::from_str(r#"{"recent":{"cwd":"/repo/engine","args":["test"],"place":"local","wall":10,"ok":false,"ago":2,"agent_session":"ours"}}"#).unwrap();
        assert!(
            snapshot
                .row(Some("other"), Some("/other"), Duration::ZERO)
                .is_none()
        );
        assert!(
            snapshot
                .row(Some("ours"), None, COMPLETION_RETENTION)
                .is_none()
        );
        let row = snapshot.row(Some("ours"), None, Duration::ZERO).unwrap();
        assert_eq!(row.tone, Tone::Failure);
        assert!(row.text.ends_with("failed in 10s"));
    }

    #[test]
    fn unknown_cores_are_not_presented_as_zero() {
        let snapshot: Snapshot = serde_json::from_str(
            r#"{"active":[{"cwd":"/repo/engine","label":"engine test","place":"local","ncpu":16}]}"#,
        )
        .unwrap();
        assert!(
            snapshot
                .row(None, Some("/repo/engine"), Duration::ZERO)
                .unwrap()
                .text
                .contains("?/16 cores")
        );
        assert_eq!(clean("evil\n\u{1b}[31m"), "evil[31m");
    }

    #[test]
    fn matching_completion_survives_unrelated_updates_and_expires() {
        let (sender, receiver) = oneshot::channel();
        sender.send(Some(serde_json::from_str::<Snapshot>(r#"{"recent":{"cwd":"/repo/app","args":["test"],"wall":51,"ok":true,"ago":1,"agent_session":"ours"}}"#).unwrap())).unwrap();
        let mut poller = Poller {
            pending: Some(receiver),
            ..Default::default()
        };
        poller.refresh(Some("ours"), Some("/repo/app"), true);
        assert!(poller.row().unwrap().text.contains("finished in 51s"));
        let (sender, receiver) = oneshot::channel();
        sender.send(Some(serde_json::from_str::<Snapshot>(r#"{"active":[{"cwd":"/other"}],"recent":{"cwd":"/other","wall":1,"ok":false,"ago":0}}"#).unwrap())).unwrap();
        poller.pending = Some(receiver);
        poller.refresh(Some("ours"), Some("/repo/app"), true);
        assert!(poller.row().unwrap().text.contains("finished in 51s"));
        poller.snapshot = None;
        poller.last_finished.as_mut().unwrap().1 = Instant::now() - Duration::from_secs(60);
        poller.refresh(Some("ours"), Some("/repo/app"), true);
        assert!(poller.row().is_some());
        poller.refresh(Some("theirs"), Some("/other"), true);
        assert!(poller.row().is_none());
        poller.last_finished.as_mut().unwrap().1 = Instant::now() - COMPLETION_RETENTION;
        poller.refresh(Some("ours"), Some("/repo/app"), true);
        assert!(poller.row().is_none());
    }

    #[test]
    fn disabled_poller_clears_cached_builds_without_starting_queries() {
        let mut poller = Poller {
            row: Some(StatusRow {
                text: "old".into(),
                tone: Tone::Active,
            }),
            ..Default::default()
        };
        assert!(poller.refresh(None, None, false));
        assert!(poller.row().is_none());
        assert!(poller.pending.is_none());
    }

    #[tokio::test]
    async fn missing_executable_is_optional() {
        assert!(
            query(std::ffi::OsStr::new("/nonexistent/justrust-status-test"))
                .await
                .is_none()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn status_subprocess_is_bounded_and_checks_exit_status() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("status");
        let write = |body: &str| {
            std::fs::write(&program, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        };
        write(
            "test \"$1\" = status && test \"$2\" = --json || exit 9\nprintf '%s' '{\"active\":[],\"recent\":null}'",
        );
        assert!(query(program.as_os_str()).await.is_some());
        write("printf '%s' '{\"active\":[]}'\nexit 1");
        assert!(query(program.as_os_str()).await.is_none());
        write("printf '%s' 'not json'");
        assert!(query(program.as_os_str()).await.is_none());
        write("exec head -c 524289 /dev/zero");
        assert!(query(program.as_os_str()).await.is_none());
        write("exec sleep 10");
        // Exercise the real subprocess cancellation path without making every
        // filtered test run wait for the production two-second deadline.
        assert_eq!(QUERY_TIMEOUT, Duration::from_secs(2));
        let timeout = Duration::from_millis(50);
        let start = Instant::now();
        assert!(
            query_with_timeout(program.as_os_str(), timeout)
                .await
                .is_none()
        );
        assert!(start.elapsed() >= timeout);
        assert!(start.elapsed() < Duration::from_secs(5));
    }
}
