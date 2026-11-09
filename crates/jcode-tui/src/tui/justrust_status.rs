//! Nonblocking consumer of justrust's public `status --json` interface.
//! This is UI-only telemetry: missing justrust or a failed query never affects
//! a tool invocation, and an SSH client must not show the laptop's builds.

use serde::Deserialize;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt as _;

const RETRY_INTERVAL: Duration = Duration::from_secs(10);
const FIRST_LINE_TIMEOUT: Duration = Duration::from_secs(5);
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
    pub own_session: bool,
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
                own_session: relevance(run.agent_session.as_deref(), &run.cwd, session, repo) == 2,
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
            own_session: relevance(run.agent_session.as_deref(), &run.cwd, session, repo) == 2,
        })
    }
}

/// One long-lived `justrust status --json --watch` per client. justrust
/// prints a line only when the status changes and sleeps on inotify while no
/// build runs, so an idle client costs nothing (a one-shot query per second
/// used to spawn a process every second in every window).
fn spawn_stream(
    program: std::ffi::OsString,
) -> tokio::sync::watch::Receiver<Option<Option<Arc<Snapshot>>>> {
    let (sender, receiver) = tokio::sync::watch::channel(None);
    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
        runtime.spawn(async move {
            if let Err(error) = stream(&program, &sender).await {
                crate::logging::debug(&format!("Build status stream ended: {error:#}"));
            }
            // Some(None) tells the poller the stream is gone.
            let _ = sender.send(Some(None));
        });
    }
    receiver
}

async fn stream(
    program: &std::ffi::OsStr,
    sender: &tokio::sync::watch::Sender<Option<Option<Arc<Snapshot>>>>,
) -> anyhow::Result<()> {
    use anyhow::Context;
    use tokio::io::AsyncBufReadExt;
    let mut child = tokio::process::Command::new(program)
        .args(["status", "--json", "--watch", "1"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("start justrust status stream")?;
    let stdout = child.stdout.take().context("stdout was not piped")?;
    let mut reader = tokio::io::BufReader::new(stdout);
    let mut line = Vec::new();
    let mut first = true;
    loop {
        line.clear();
        let limit = if first {
            FIRST_LINE_TIMEOUT
        } else {
            Duration::from_secs(365 * 24 * 3600)
        };
        let mut limited = (&mut reader).take((MAX_RESPONSE + 1) as u64);
        let next = limited.read_until(b'\n', &mut line);
        let read = tokio::select! {
            read = tokio::time::timeout(limit, next) => {
                read.context("first status line timed out")?
            }
            () = sender.closed() => return Ok(()),
        };
        first = false;
        let n = read.context("read justrust status stream")?;
        anyhow::ensure!(n > 0, "justrust status stream closed");
        anyhow::ensure!(
            line.ends_with(b"\n") || n <= MAX_RESPONSE,
            "justrust status line exceeds {MAX_RESPONSE} bytes"
        );
        let snapshot: Snapshot =
            serde_json::from_slice(&line).context("decode justrust status line")?;
        if sender.send(Some(Some(Arc::new(snapshot)))).is_err() {
            return Ok(());
        }
    }
}

#[derive(Default)]
pub(crate) struct Poller {
    stream: Option<tokio::sync::watch::Receiver<Option<Option<Arc<Snapshot>>>>>,
    snapshot: Option<Arc<Snapshot>>,
    received: Option<Instant>,
    next_start: Option<Instant>,
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
            self.stream = None;
            return self.row != previous;
        }
        let now = Instant::now();
        let mut ended = false;
        if let Some(receiver) = &mut self.stream
            && receiver.has_changed().unwrap_or(true)
        {
            match receiver.borrow_and_update().clone() {
                Some(Some(snapshot)) => {
                    if let Some(run) = snapshot.recent.as_ref()
                        && relevance(run.agent_session.as_deref(), &run.cwd, session, repo) > 0
                    {
                        self.last_finished = Some((run.clone(), now));
                    }
                    self.snapshot = Some(snapshot);
                    self.received = Some(now);
                }
                Some(None) => ended = true,
                None => {}
            }
        }
        if ended {
            self.stream = None;
            self.snapshot = None;
            self.next_start = Some(now + RETRY_INTERVAL);
        }
        // A live stream's snapshot stays current until the next line: an
        // active build prints every second, an idle one never needs to.
        self.row = self.snapshot.as_ref().and_then(|snapshot| {
            let age = self
                .received
                .map_or(Duration::ZERO, |at| now.saturating_duration_since(at));
            let fresh = !snapshot.active.is_empty() && age < Duration::from_secs(5)
                || snapshot.active.is_empty();
            fresh.then(|| snapshot.row(session, repo, age)).flatten()
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
        if self.stream.is_none()
            && self.next_start.is_none_or(|next| now >= next)
            && tokio::runtime::Handle::try_current().is_ok()
        {
            self.stream = Some(spawn_stream("justrust".into()));
            self.next_start = Some(now + RETRY_INTERVAL);
        }
        self.row != previous
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ownership_distinguishes_session_from_repository_for_all_outcomes() {
        for active in [true, false] {
            for ok in [true, false] {
                for owner in ["ours", "other"] {
                    let run = serde_json::json!({
                        "cwd": "/repo/app", "agent_session": owner,
                        "wall": 5, "ago": 1, "ok": ok
                    });
                    let value = if active {
                        serde_json::json!({"active": [run]})
                    } else {
                        serde_json::json!({"recent": run})
                    };
                    let snapshot: Snapshot = serde_json::from_value(value).unwrap();
                    let row = snapshot
                        .row(Some("ours"), Some("/repo/app"), Duration::ZERO)
                        .unwrap();
                    assert_eq!(row.own_session, owner == "ours");
                    assert_eq!(
                        row.tone,
                        if active {
                            Tone::Active
                        } else if ok {
                            Tone::Success
                        } else {
                            Tone::Failure
                        }
                    );
                }
            }
        }
    }

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

    fn feed(json: &str) -> tokio::sync::watch::Receiver<Option<Option<Arc<Snapshot>>>> {
        let snapshot = serde_json::from_str::<Snapshot>(json).unwrap();
        let (sender, receiver) = tokio::sync::watch::channel(Some(Some(Arc::new(snapshot))));
        std::mem::forget(sender);
        receiver
    }

    #[test]
    fn matching_completion_survives_unrelated_updates_and_expires() {
        let mut poller = Poller {
            stream: Some(feed(
                r#"{"recent":{"cwd":"/repo/app","args":["test"],"wall":51,"ok":true,"ago":1,"agent_session":"ours"}}"#,
            )),
            ..Default::default()
        };
        poller.stream.as_mut().unwrap().mark_changed();
        poller.refresh(Some("ours"), Some("/repo/app"), true);
        assert!(poller.row().unwrap().text.contains("finished in 51s"));
        let mut next = feed(
            r#"{"active":[{"cwd":"/other"}],"recent":{"cwd":"/other","wall":1,"ok":false,"ago":0}}"#,
        );
        next.mark_changed();
        poller.stream = Some(next);
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
    fn idle_snapshot_stays_current_without_new_lines() {
        let mut poller = Poller {
            stream: Some(feed(
                r#"{"active":[],"recent":{"cwd":"/repo/app","args":["test"],"wall":3,"ok":true,"ago":1,"agent_session":"ours"}}"#,
            )),
            ..Default::default()
        };
        poller.stream.as_mut().unwrap().mark_changed();
        poller.refresh(Some("ours"), Some("/repo/app"), true);
        poller.received = Some(Instant::now() - Duration::from_secs(30));
        poller.last_finished = None;
        poller.refresh(Some("ours"), Some("/repo/app"), true);
        assert!(poller.row().unwrap().text.contains("finished in 3s"));
    }

    #[test]
    fn disabled_poller_clears_cached_builds_without_starting_queries() {
        let mut poller = Poller {
            row: Some(StatusRow {
                text: "old".into(),
                tone: Tone::Active,
                own_session: false,
            }),
            ..Default::default()
        };
        assert!(poller.refresh(None, None, false));
        assert!(poller.row().is_none());
        assert!(poller.stream.is_none());
    }

    async fn first(program: &std::path::Path) -> Option<Option<Arc<Snapshot>>> {
        let mut receiver = spawn_stream(program.as_os_str().to_owned());
        receiver.changed().await.ok()?;
        receiver.borrow().clone()
    }

    #[tokio::test]
    async fn missing_executable_is_optional() {
        assert!(matches!(
            first(std::path::Path::new("/nonexistent/justrust-status-test")).await,
            Some(None) | None
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn status_stream_is_bounded_and_streams_lines() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("status");
        let write = |body: &str| {
            std::fs::write(&program, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        };
        write(
            "test \"$1 $2 $3\" = 'status --json --watch' || exit 9\necho '{\"active\":[],\"recent\":null}'\nsleep 0.2\necho '{\"active\":[{\"cwd\":\"/x\"}]}'\nsleep 10",
        );
        let mut receiver = spawn_stream(program.as_os_str().to_owned());
        receiver.changed().await.unwrap();
        assert!(
            receiver
                .borrow_and_update()
                .clone()
                .unwrap()
                .unwrap()
                .active
                .is_empty()
        );
        receiver.changed().await.unwrap();
        assert_eq!(
            receiver
                .borrow_and_update()
                .clone()
                .unwrap()
                .unwrap()
                .active
                .len(),
            1
        );
        drop(receiver);
        write("echo 'not json'");
        assert!(matches!(first(&program).await, Some(None)));
        write("exec head -c 524289 /dev/zero");
        assert!(matches!(first(&program).await, Some(None)));
        write("exit 1");
        assert!(matches!(first(&program).await, Some(None)));
    }
}
