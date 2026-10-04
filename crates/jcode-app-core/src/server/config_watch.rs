//! Daemon-side detection of global config changes, so a config edit is
//! visible to every session instead of only the one that made it.
//!
//! `~/.jcode/config.toml` is deliberately global (see AGENTS.md "Project
//! Isolation Invariants"): one file holds model/tool policy, MCP definitions,
//! and auth settings for every project the daemon serves. That is a documented
//! product decision, and this module exists so the consequence of it is
//! visible rather than silent.
//!
//! Why this lives in the daemon rather than in the file tools: the file tools
//! already report a config edit, but only in the tool result body, so only the
//! session that wrote the file learns about it. A gate or a notice at the file
//! tool layer would also be trivially bypassable, because `bash` can write the
//! same file (`echo '...' >> ~/.jcode/config.toml`) and `bash.rs` has no
//! concept of config at all. Watching the file itself is the one place that
//! sees every writer: the file tools, `bash`, a user's editor, and any other
//! process on the machine.
//!
//! Deliberately one mechanism, not two. The per-tool notice in
//! `tool/config_edit_notice.rs` stays, because it tells the writing session
//! exactly which keys it changed and whether each one is live. This notifies
//! the sessions that did *not* write it. Collapsing them would lose one or the
//! other; keeping both means each answers a different question.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;

use super::state::{SwarmMember, fanout_session_event};
use crate::protocol::{NotificationType, ServerEvent};

/// How often the daemon re-reads the config file to notice a change.
///
/// Polling rather than a filesystem-watcher dependency: the workspace has no
/// `notify` crate, and one `stat` plus a read of a small TOML file every few
/// seconds is not worth a new dependency and its platform quirks. This is the
/// same tradeoff `Config`'s own reload cache already makes, so the two agree
/// on when a change becomes visible.
const CONFIG_WATCH_INTERVAL: Duration = Duration::from_secs(5);

/// Scope tag on the emitted notification.
///
/// The TUI already dispatches on scope strings for other daemon notices, so
/// reusing the mechanism costs no wire change and no new rendering path.
const CONFIG_SCOPE: &str = "config_change";

/// What the watcher knows about the config file as of the last tick.
///
/// Three states rather than an `Option<String>`, because "no baseline yet" and
/// "the file does not exist" need opposite answers. Collapsing them means the
/// very first write of a config.toml is absorbed as a baseline and never
/// reported, which is exactly the case where a user who never edited their
/// config ends up with one applied in every project without being told.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ConfigState {
    /// Nothing observed yet, because the watcher just started.
    ///
    /// An existing file here is the daemon's own baseline, not a change.
    Unobserved,
    /// The path was observed and held no file.
    Absent,
    /// The path was observed with this content.
    Content(String),
}

/// State the watcher owns between ticks.
pub(super) struct ConfigWatch {
    state: ConfigState,
    /// Resolved config path observed at the last tick.
    ///
    /// `JCODE_HOME` can move between ticks in tests and in sandboxes, so the
    /// path is re-resolved when it no longer matches what we were watching
    /// rather than pinned forever.
    path: Option<PathBuf>,
}

impl ConfigWatch {
    pub(super) fn new() -> Self {
        Self {
            state: ConfigState::Unobserved,
            path: None,
        }
    }
}

/// What one tick observed.
pub(super) enum ConfigTick {
    /// No change worth reporting since the previous tick.
    Unchanged,
    /// The file changed, with a rendered summary and the path involved.
    Changed { path: PathBuf, summary: String },
}

/// Advance the watcher by one tick, returning what it observed.
///
/// Kept separate from delivery so the decision "did the global config change,
/// and is it worth telling sessions about" is testable without a swarm, a
/// server, or a session attached.
///
/// Every transition that changes what a session's `config()` returns is
/// reported: a file appearing, content changing, and the file disappearing.
/// The last one is not a formatting detail. Deleting config.toml reverts every
/// session's model, tool, and MCP settings to defaults at once, in every
/// project, which is the same silent global behavior change as a bad edit.
pub(super) fn tick(watch: &mut ConfigWatch) -> ConfigTick {
    let Some(config_path) = crate::config::Config::path() else {
        return ConfigTick::Unchanged;
    };

    // `JCODE_HOME` moved under us (tests, sandbox switching). Treat it as a new
    // target so the new file gets a fresh baseline instead of being diffed
    // against the old location's content.
    if let Some(watched) = &watch.path
        && crate::tool::config_comparable_path(watched)
            != crate::tool::config_comparable_path(&config_path)
    {
        watch.state = ConfigState::Unobserved;
    }
    watch.path = Some(config_path.clone());

    let current = std::fs::read_to_string(&config_path).ok();
    let previous = std::mem::replace(
        &mut watch.state,
        match &current {
            Some(content) => ConfigState::Content(content.clone()),
            None => ConfigState::Absent,
        },
    );

    let report = |summary: String| ConfigTick::Changed {
        path: config_path.clone(),
        summary,
    };

    match (&previous, &current) {
        // First tick ever: whatever is on disk is the baseline, not a change.
        (ConfigState::Unobserved, _) => ConfigTick::Unchanged,
        // Still no file, and never was one at this path.
        (ConfigState::Absent, None) => ConfigTick::Unchanged,
        // The config just appeared. Every session's settings changed at once,
        // including projects that never asked for it.
        (ConfigState::Absent, Some(_)) => report(format!(
            "{path} now exists, so every session in this daemon is running its \
             settings for the first time. Settings that were falling back to \
             defaults are now being applied.",
            path = config_path.display(),
        )),
        // The config just disappeared, so every session falls back to defaults.
        (ConfigState::Content(_), None) => report(format!(
            "{path} no longer exists, so every session in this daemon has fallen \
             back to default model, tool, and MCP settings.",
            path = config_path.display(),
        )),
        (ConfigState::Content(previous), Some(content)) => {
            if previous == content {
                return ConfigTick::Unchanged;
            }
            // Force the next config() read rather than waiting out the
            // staleness throttle, so a session is never told about a change it
            // has not loaded yet.
            crate::config::Config::invalidate_cache();

            // A config that no longer parses is the case that most needs
            // reporting: `Config::load` falls back to defaults, so the write
            // "succeeded" while every setting in the file quietly stopped
            // applying.
            if let Err(error) = crate::config::Config::load_strict() {
                return report(format!(
                    "{error} - jcode is falling back to default settings, so every \
                     setting in this file is currently being ignored. Fix the syntax \
                     error before relying on any of them."
                ));
            }

            // `summarize_toml_change` returns `None` for edits that changed no
            // setting (comments, formatting), so those stay silent.
            match crate::config::change_report::summarize_toml_change(previous, content) {
                Some(summary) => report(summary),
                None => ConfigTick::Unchanged,
            }
        }
    }
}

/// Notify every session that the global config changed.
///
/// `config.toml` is shared by all projects by design, so every session gets the
/// notice, including the one that caused it. Scoping this to the writing
/// session would be exactly the bug this replaces: the sessions that did *not*
/// write it are the ones whose behavior silently changed.
///
/// `from_session` is `"jcode"` rather than a session id because the writer is
/// frequently not a session at all (an editor, a `bash` command, another
/// process). Naming a session that did not do it would be a lie, and naming
/// none would leave the notice unattributed.
pub(super) async fn notify_all_sessions(
    path: &Path,
    summary: String,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
) -> usize {
    let targets: Vec<String> = {
        let members = swarm_members.read().await;
        members.keys().cloned().collect()
    };

    let message = format!(
        "jcode's global config.toml ({}) changed, which affects every session in this \
         daemon including projects other than this one: {summary}",
        path.display()
    );

    let mut delivered = 0;
    for session_id in targets {
        delivered += fanout_session_event(
            swarm_members,
            &session_id,
            ServerEvent::Notification {
                from_session: "jcode".to_string(),
                from_name: Some("Jcode".to_string()),
                notification_type: NotificationType::Message {
                    scope: Some(CONFIG_SCOPE.to_string()),
                    channel: None,
                    tldr: Some(format!(
                        "global {} changed for every session in this daemon",
                        path.file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_else(|| "config".to_string())
                    )),
                },
                message: message.clone(),
            },
        )
        .await;
    }
    delivered
}

/// Watch the global config until the process exits.
///
/// One tick per [`CONFIG_WATCH_INTERVAL`], independent of bus traffic, so a
/// change made with no jcode activity at all is still noticed. That
/// independence is the reason this is its own task rather than another arm of
/// `monitor_bus`, whose loop is blocked on `receiver.recv()` and would only run
/// this check when some other event happened to arrive first.
pub(super) async fn watch_config_file(swarm_members: Arc<RwLock<HashMap<String, SwarmMember>>>) {
    watch_config_file_every(swarm_members, CONFIG_WATCH_INTERVAL).await;
}

/// The watch loop, with the interval injected so a test can drive it.
///
/// The first tick runs immediately rather than after the first sleep: a daemon
/// that just started should establish its baseline before any change is
/// mistaken for one, and there is no reason to make that wait a full interval.
async fn watch_config_file_every(
    swarm_members: Arc<RwLock<HashMap<String, SwarmMember>>>,
    interval: Duration,
) {
    let mut watch = ConfigWatch::new();
    loop {
        if let ConfigTick::Changed { path, summary } = tick(&mut watch) {
            notify_all_sessions(&path, summary, &swarm_members).await;
        }
        tokio::time::sleep(interval).await;
    }
}

#[cfg(test)]
#[path = "config_watch_tests.rs"]
mod tests;
