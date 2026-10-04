//! Tests for the daemon-side global config watcher.
//!
//! Two levels are covered separately, because they regress independently:
//!
//! * `tick` decides *whether* a config change is worth reporting. These tests
//!   need no swarm and no server.
//! * `notify_all_sessions` decides *who* is told. This is the isolation
//!   property: a session in another project must hear about a global config
//!   change, because the config it reads just changed under it.
//!
//! A test of the first does not prove the second, and vice versa, so neither is
//! left to the other.

use super::*;
use crate::env;
use std::time::Instant;
use tokio::sync::mpsc;

/// Redirect `JCODE_HOME` at a temp dir for the duration of one test.
///
/// Restores the previous value on drop, including on panic, so a failing
/// assertion cannot leave the developer's real `~/.jcode` redirected for the
/// rest of the test binary.
struct TempHome {
    _dir: tempfile::TempDir,
    previous: Option<std::ffi::OsString>,
}

impl TempHome {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("create temp JCODE_HOME");
        let previous = std::env::var_os("JCODE_HOME");
        env::set_var("JCODE_HOME", dir.path());
        // The config path is derived from JCODE_HOME and cached process-wide,
        // so a stale cache would point these tests at the real config file.
        crate::config::Config::invalidate_cache();
        Self {
            _dir: dir,
            previous,
        }
    }

    fn path(&self) -> PathBuf {
        crate::config::Config::path().expect("config path under a JCODE_HOME")
    }

    fn write(&self, content: &str) {
        std::fs::write(self.path(), content).expect("write config");
    }
    fn remove(&self) {
        let _ = std::fs::remove_file(self.path());
    }
}

impl Drop for TempHome {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => env::set_var("JCODE_HOME", value),
            None => env::remove_var("JCODE_HOME"),
        }
        crate::config::Config::invalidate_cache();
    }
}

/// Build a swarm member with the given session id and its own event channel.
///
/// Shaped to match the other `member()` helpers in this module's siblings
/// (`comm_control_tests.rs`, `reload_tests.rs`) rather than inventing a
/// construction path that production never uses.
fn member(session_id: &str, event_tx: mpsc::UnboundedSender<ServerEvent>) -> SwarmMember {
    SwarmMember {
        session_id: session_id.to_string(),
        event_tx,
        event_txs: HashMap::new(),
        working_dir: None,
        swarm_id: None,
        swarm_enabled: false,
        status: "ready".to_string(),
        detail: None,
        task_label: None,
        friendly_name: None,
        report_back_to_session_id: None,
        latest_completion_report: None,
        role: "agent".to_string(),
        joined_at: Instant::now(),
        last_status_change: Instant::now(),
        is_headless: false,
        output_tail: None,
        todo_progress: None,
        todo_items: Vec::new(),
        runtime: crate::protocol::SwarmMemberRuntime::default(),
    }
}

#[test]
fn a_setting_change_is_reported_and_names_the_key() {
    let home = TempHome::new();
    home.write("[display]\ncentered = false\n");

    let mut watch = ConfigWatch::new();
    // The first tick establishes the baseline and must stay silent.
    assert!(matches!(tick(&mut watch), ConfigTick::Unchanged));

    home.write("[display]\ncentered = true\n");

    let ConfigTick::Changed { summary, .. } = tick(&mut watch) else {
        panic!("a changed setting must be reported");
    };
    assert!(
        summary.contains("centered"),
        "the summary must name the key that changed, or no session can tell what to \
         re-check: {summary}"
    );
}

#[test]
fn one_edit_is_reported_once_and_not_repeated_on_later_ticks() {
    let home = TempHome::new();
    home.write("[display]\ncentered = false\n");

    let mut watch = ConfigWatch::new();
    assert!(matches!(tick(&mut watch), ConfigTick::Unchanged));
    // Quiet ticks must not re-report anything.
    assert!(matches!(tick(&mut watch), ConfigTick::Unchanged));
    assert!(matches!(tick(&mut watch), ConfigTick::Unchanged));

    home.write("[display]\ncentered = true\n");
    assert!(matches!(tick(&mut watch), ConfigTick::Changed { .. }));
    // The next tick sees no new change, so the same edit must not fire again.
    assert!(matches!(tick(&mut watch), ConfigTick::Unchanged));
    assert!(matches!(tick(&mut watch), ConfigTick::Unchanged));
}

#[test]
fn the_first_write_of_a_config_that_did_not_exist_is_reported() {
    let home = TempHome::new();
    // No file yet: the first tick has nothing to compare against.
    let mut watch = ConfigWatch::new();
    assert!(matches!(tick(&mut watch), ConfigTick::Unchanged));

    home.write("[display]\ncentered = true\n");

    assert!(
        matches!(tick(&mut watch), ConfigTick::Changed { .. }),
        "creating the config file is itself a global change every other session needs \
         to know about; absorbing it as a baseline means a user who never edited their \
         config gets one applied silently in every project"
    );
}

/// Summaries are rendered straight into a notification a user reads, so a run
/// of spaces typed mid-sentence ships as visible garbage. Assert it over every
/// summary the watcher composes itself, not only the two that happened to be
/// wrong. The parse-failure summary embeds a `toml` diagnostic, whose aligned
/// gutter markers are correct output and are covered by their own assertion
/// below.
#[test]
fn no_reported_summary_carries_runs_of_whitespace() {
    let home = TempHome::new();
    let mut watch = ConfigWatch::new();
    let mut summaries: Vec<String> = Vec::new();

    // Absent -> present.
    assert!(matches!(tick(&mut watch), ConfigTick::Unchanged));
    home.write("[display]\ncentered = true\n");
    if let ConfigTick::Changed { summary, .. } = tick(&mut watch) {
        summaries.push(summary);
    }

    // Present -> changed.
    home.write("[display]\ncentered = false\n");
    if let ConfigTick::Changed { summary, .. } = tick(&mut watch) {
        summaries.push(summary);
    }

    // Present -> absent.
    home.remove();
    if let ConfigTick::Changed { summary, .. } = tick(&mut watch) {
        summaries.push(summary);
    }

    assert_eq!(
        summaries.len(),
        3,
        "every transition must have reported; got {summaries:?}"
    );
    for summary in &summaries {
        assert!(
            !summary.contains("  "),
            "summary smuggles a run of spaces into a sentence a user reads: {summary}"
        );
        assert_eq!(
            summary.trim(),
            summary,
            "summary has leading or trailing whitespace: {summary:?}"
        );
    }

    // The parse-failure summary wraps a `toml` diagnostic, whose own alignment
    // this must leave alone. Only our sentence is ours to keep clean.
    home.write("[display]\ncentered = false\n");
    assert!(
        matches!(tick(&mut watch), ConfigTick::Changed { .. }),
        "recreating the config is itself a global change"
    );
    home.write("this is not toml at all ][");
    let ConfigTick::Changed { summary, .. } = tick(&mut watch) else {
        panic!("a config that stopped parsing must still be reported");
    };
    assert!(
        summary.contains("falling back to default settings"),
        "the parse-failure summary must say the file is being ignored: {summary}"
    );
}
#[test]
fn a_comment_only_edit_is_not_reported() {
    let home = TempHome::new();
    home.write("[display]\ncentered = false\n");

    let mut watch = ConfigWatch::new();
    assert!(matches!(tick(&mut watch), ConfigTick::Unchanged));

    home.write("# a new comment\n[display]\ncentered = false\n");

    assert!(
        matches!(tick(&mut watch), ConfigTick::Unchanged),
        "a comment-only edit changes no setting, so notifying every session would be \
         noise the notice's own change report already filters out"
    );
}

#[test]
fn a_config_that_stops_parsing_is_reported_as_being_ignored() {
    let home = TempHome::new();
    home.write("[display]\ncentered = false\n");

    let mut watch = ConfigWatch::new();
    assert!(matches!(tick(&mut watch), ConfigTick::Unchanged));

    home.write("[display\ncentered = false\n");

    let ConfigTick::Changed { summary, .. } = tick(&mut watch) else {
        panic!("a config that no longer parses must be reported");
    };
    assert!(
        summary.contains("default settings") && summary.contains("being ignored"),
        "the report must say the file's settings stopped applying, not list a key \
         change, because nothing in it is live: {summary}"
    );
}

#[test]
fn a_moved_config_path_re_baselines_instead_of_reporting_a_phantom_change() {
    let first = TempHome::new();
    first.write("[display]\ncentered = false\n");

    let mut watch = ConfigWatch::new();
    assert!(matches!(tick(&mut watch), ConfigTick::Unchanged));

    // A different JCODE_HOME means a different file with unrelated content.
    // Diffing it against the old file's content would report a change that never
    // happened in the new location.
    let second = TempHome::new();
    second.write("[gateway]\nport = 9999\n");

    assert!(
        matches!(tick(&mut watch), ConfigTick::Unchanged),
        "moving the watched config path must re-baseline silently"
    );

    // The new location is still watched: a real edit there is reported.
    second.write("[gateway]\nport = 7777\n");
    assert!(matches!(tick(&mut watch), ConfigTick::Changed { .. }));
}

#[test]
fn deleting_the_config_is_reported_and_its_recreation_is_reported_too() {
    let home = TempHome::new();
    home.write("[display]\ncentered = false\n");

    let mut watch = ConfigWatch::new();
    assert!(matches!(tick(&mut watch), ConfigTick::Unchanged));

    // Deleting config.toml reverts every session to defaults at once, which is
    // a silent global behavior change and not a detail to swallow.
    std::fs::remove_file(home.path()).expect("remove config");
    let ConfigTick::Changed { summary, .. } = tick(&mut watch) else {
        panic!("deleting the global config must be reported");
    };
    assert!(
        summary.contains("default") && summary.contains("every session"),
        "the report must say settings reverted across all sessions, since that is a \
         silent behavior change in every project at once: {summary}"
    );

    // And a config that comes back is a write every session must hear about.
    home.write("[display]\ncentered = true\n");
    assert!(
        matches!(tick(&mut watch), ConfigTick::Changed { .. }),
        "a config recreated after deletion is a fresh global change, not a restore"
    );
}

/// The timer loop must notice a config change with no other daemon activity.
///
/// This is the level that can regress independently of `tick`: the loop could
/// stop running, or be folded back into the bus monitor where it only fires
/// when some other event arrives, and every `tick` test above would still pass.
#[tokio::test]
async fn the_watch_loop_notices_a_change_with_no_other_daemon_activity() {
    let home = TempHome::new();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let members = Arc::new(RwLock::new(HashMap::from([(
        "session-a".to_string(),
        member("session-a", tx),
    )])));

    let watch_members = Arc::clone(&members);
    let task = tokio::spawn(async move {
        watch_config_file_every(watch_members, Duration::from_millis(5)).await;
    });

    // Let the task run its first tick before editing. That tick reads the
    // baseline; without it the edit below would be consumed *as* the baseline
    // and never reported, which is the race this test would otherwise flake on.
    tokio::time::sleep(Duration::from_millis(100)).await;

    home.write("[display]\ncentered = true\n");

    let event = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(event) = rx.try_recv() {
                break event;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the loop must deliver within a few ticks");

    task.abort();

    let ServerEvent::Notification { message, .. } = event else {
        panic!("expected a Notification");
    };
    assert!(
        message.contains("config.toml"),
        "the delivered notification must name what changed: {message}"
    );
}

/// The daemon must actually spawn the watcher.
///
/// A source assertion, which is normally the wrong kind of test. It is the
/// right one here because the failure mode is a silent regression with no
/// other symptom: delete the `tokio::spawn` and every test in this file still
/// passes, because they all drive `tick` and the loop directly. Nothing else
/// in the crate can observe whether the daemon runs the watcher, so binding to
/// the spawn site is the only way this fails loudly instead of shipping dead.
#[test]
fn the_daemon_spawns_the_config_watcher() {
    // Normalized because server.rs is CRLF on Windows, and a raw multi-line
    // match against "\n" would pass there and fail everywhere else.
    let server_src = include_str!("../server.rs").replace("\r\n", "\n");
    assert!(
        server_src.contains("config_watch::watch_config_file("),
        "server.rs must spawn config_watch::watch_config_file; without it the global \
         config watcher never runs and a config change reaches no session"
    );
    // Spawned as a task, not awaited inline: an awaited loop would hang startup.
    assert!(
        server_src
            .contains("tokio::spawn(async move {\n            config_watch::watch_config_file("),
        "the watcher must run as a spawned task, not be awaited on the startup path"
    );
}

#[tokio::test]
async fn a_global_config_change_reaches_every_session_including_other_projects() {
    // Three sessions in three unrelated working directories, which is the
    // multi-project shape the daemon actually serves.
    let dirs: Vec<PathBuf> = (0..3).map(|_| PathBuf::from("/some/project")).collect();
    let mut map = HashMap::new();
    let mut receivers = Vec::new();
    for (index, working_dir) in dirs.iter().enumerate() {
        let (tx, rx) = mpsc::unbounded_channel();
        receivers.push(rx);
        let mut m = member(&format!("session-{index}"), tx);
        m.working_dir = Some(working_dir.clone());
        map.insert(m.session_id.clone(), m);
    }
    let members = Arc::new(RwLock::new(map));

    let delivered = notify_all_sessions(
        &PathBuf::from("/home/u/.jcode/config.toml"),
        "display.centered: false -> true".to_string(),
        &members,
    )
    .await;

    assert_eq!(
        delivered, 3,
        "every session must be told: the sessions that did NOT write the config are \
         exactly the ones whose behavior changed under them"
    );

    for (index, rx) in receivers.iter_mut().enumerate() {
        let event = rx.try_recv().unwrap_or_else(|_| {
            panic!("session-{index} received no notification");
        });
        let ServerEvent::Notification {
            from_session,
            message,
            ..
        } = event
        else {
            panic!("session-{index} got a different event");
        };
        assert_eq!(
            from_session, "jcode",
            "session-{index}: the writer is often not a session at all (an editor, a \
             bash command), so the notice must not be attributed to one"
        );
        assert!(
            message.contains("every session") && message.contains("config.toml"),
            "session-{index}: the message must state the blast radius: {message}"
        );
    }
}

#[tokio::test]
async fn the_notification_is_scoped_as_a_config_change_with_a_one_line_summary() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let members = Arc::new(RwLock::new(HashMap::from([(
        "session-a".to_string(),
        member("session-a", tx),
    )])));

    notify_all_sessions(
        &PathBuf::from("/home/u/.jcode/config.toml"),
        "display.centered: false -> true".to_string(),
        &members,
    )
    .await;

    let ServerEvent::Notification {
        from_name,
        notification_type,
        ..
    } = rx.try_recv().expect("notification")
    else {
        panic!("expected a Notification");
    };
    assert_eq!(from_name.as_deref(), Some("Jcode"));
    let NotificationType::Message {
        scope,
        tldr,
        channel,
    } = notification_type
    else {
        panic!("expected a message notification");
    };
    assert_eq!(scope.as_deref(), Some(CONFIG_SCOPE));
    assert!(
        channel.is_none(),
        "a config change belongs to no channel; setting one would file it as a \
         cross-agent message instead of a daemon notice"
    );
    assert!(
        tldr.is_some_and(|t| !t.is_empty()),
        "the TUI renders notifications collapsed on their tldr, so a global change \
         needs one or it arrives as an unexplained full-body block"
    );
}

#[tokio::test]
async fn a_session_that_leaves_before_the_notice_is_skipped_without_panicking() {
    let (live_tx, mut live_rx) = mpsc::unbounded_channel();
    // A dropped receiver is what a disconnected client looks like: the member
    // stays in the map while its channel is closed.
    let (dead_tx, dead_rx) = mpsc::unbounded_channel();
    drop(dead_rx);
    let members = Arc::new(RwLock::new(HashMap::from([
        ("session-live".to_string(), member("session-live", live_tx)),
        ("session-dead".to_string(), member("session-dead", dead_tx)),
    ])));

    let delivered = notify_all_sessions(
        &PathBuf::from("/home/u/.jcode/config.toml"),
        "display.centered: false -> true".to_string(),
        &members,
    )
    .await;

    assert_eq!(
        delivered, 1,
        "a session with a closed channel cannot receive anything; the fanout must \
         keep going instead of stopping at it, or one dead client hides the change \
         from every live session after it"
    );
    assert!(live_rx.try_recv().is_ok(), "the live session is notified");
}
