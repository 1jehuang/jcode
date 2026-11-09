//! Running-turn presence for sessions owned by a cloud host.
//!
//! Local presence comes from `~/.jcode/streaming_pids`. Cloud sessions keep
//! the same markers on their VM, so one bounded SSH read per host reports
//! which turns are running there. Never runs on the render thread.
use super::*;
use std::sync::mpsc::{Receiver, TryRecvError};

const REFRESH_INTERVAL: Duration = Duration::from_secs(10);

/// Lists `<session_id> <marker mtime>` for markers whose owner is alive.
const SCRIPT: &str = r#"d="$HOME/.jcode/streaming_pids"
[ -d "$d" ] || exit 0
for f in "$d"/*; do
  [ -f "$f" ] || continue
  p=$(cat "$f" 2>/dev/null)
  case "$p" in ''|*[!0-9]*) continue ;; esac
  kill -0 "$p" 2>/dev/null || continue
  printf '%s %s\n' "$(basename "$f")" "$(stat -c %Y "$f" 2>/dev/null || echo 0)"
done
"#;

#[derive(Default)]
pub(super) struct CloudPresence {
    /// Running cloud sessions from the last completed poll, by session id.
    pub(super) streaming: HashMap<String, Option<std::time::SystemTime>>,
    pending: Option<Receiver<HashMap<String, Option<std::time::SystemTime>>>>,
    started_at: Option<std::time::Instant>,
}

pub(super) fn parse(output: &str) -> HashMap<String, Option<std::time::SystemTime>> {
    output
        .lines()
        .filter_map(|line| {
            let (id, mtime) = line.trim().split_once(' ')?;
            let valid = !id.is_empty()
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
            valid.then(|| {
                let since = mtime
                    .trim()
                    .parse::<u64>()
                    .ok()
                    .filter(|secs| *secs > 0)
                    .map(|secs| std::time::UNIX_EPOCH + Duration::from_secs(secs));
                (id.to_string(), since)
            })
        })
        .collect()
}

fn poll_host(host: &str) -> Option<HashMap<String, Option<std::time::SystemTime>>> {
    if host.is_empty() || host.starts_with('-') {
        return None;
    }
    let config = crate::storage::jcode_dir().ok()?.join("cloud/ssh_config");
    let command = crate::tui::cloud_git::script_command(
        Some(host),
        config.is_file().then_some(config.as_path()),
        ".",
        SCRIPT,
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .ok()?;
    let (status, bytes) = runtime.block_on(crate::tui::cloud_git::bounded_output_for(
        command,
        Duration::from_secs(10),
        64 * 1024,
        crate::tui::cloud_git::WorkspaceOperation::CloudStatus,
    ))?;
    status
        .success()
        .then(|| parse(&String::from_utf8_lossy(&bytes)))
}

impl SessionPicker {
    fn cloud_hosts(&self) -> Vec<String> {
        let mut hosts: Vec<String> = self
            .all_sessions
            .iter()
            .filter(|session| session.server_icon.as_deref() == Some("☁"))
            .filter_map(|session| {
                session
                    .server_name
                    .as_deref()?
                    .strip_prefix("cloud: ")
                    .map(str::to_string)
            })
            .collect();
        hosts.sort();
        hosts.dedup();
        hosts
    }

    /// Collect a finished poll and start the next one when due. Returns true
    /// when the set of running cloud sessions changed.
    pub(super) fn maybe_refresh_cloud_presence(&mut self) -> bool {
        let mut changed = false;
        if let Some(receiver) = self.cloud_presence.pending.as_ref() {
            match receiver.try_recv() {
                Ok(streaming) => {
                    changed = streaming != self.cloud_presence.streaming;
                    self.cloud_presence.streaming = streaming;
                    self.cloud_presence.pending = None;
                }
                Err(TryRecvError::Disconnected) => self.cloud_presence.pending = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        let due = self
            .cloud_presence
            .started_at
            .is_none_or(|at| at.elapsed() >= REFRESH_INTERVAL);
        if self.cloud_presence.pending.is_none() && due {
            let hosts = self.cloud_hosts();
            self.cloud_presence.started_at = Some(std::time::Instant::now());
            if hosts.is_empty() {
                if !self.cloud_presence.streaming.is_empty() {
                    self.cloud_presence.streaming.clear();
                    changed = true;
                }
            } else {
                let (tx, rx) = std::sync::mpsc::channel();
                let spawned = std::thread::Builder::new()
                    .name("jcode-cloud-presence".into())
                    .spawn(move || {
                        let mut all = HashMap::new();
                        for host in hosts {
                            if let Some(found) = poll_host(&host) {
                                all.extend(found);
                            }
                        }
                        let _ = tx.send(all);
                    });
                if spawned.is_ok() {
                    self.cloud_presence.pending = Some(rx);
                }
            }
        }
        if changed {
            self.merge_cloud_presence();
            if self.filter_mode == SessionFilterMode::Active {
                self.rebuild_items();
            }
        }
        changed
    }

    /// Overlay remote running turns onto the local presence snapshot.
    pub(super) fn merge_cloud_presence(&mut self) {
        self.live_presence
            .retain(|_, presence| !(presence.pid == 0 && presence.streaming));
        for (session_id, since) in &self.cloud_presence.streaming {
            let is_cloud = self.all_sessions.iter().any(|session| {
                session.id == *session_id && session.server_icon.as_deref() == Some("☁")
            });
            if !is_cloud {
                continue;
            }
            self.live_presence.insert(
                session_id.clone(),
                crate::session::SessionPresence {
                    session_id: session_id.clone(),
                    pid: 0,
                    streaming: true,
                    streaming_since: *since,
                    internal: false,
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_live_markers_and_rejects_unsafe_ids() {
        let parsed = parse("session_a_1 1700000000\nbad/id 5\nsession_b_2 0\n\n");
        assert_eq!(parsed.len(), 2);
        assert_eq!(
            parsed["session_a_1"],
            Some(std::time::UNIX_EPOCH + Duration::from_secs(1_700_000_000))
        );
        assert_eq!(parsed["session_b_2"], None);
    }

    #[test]
    fn cloud_running_session_shows_running_state() {
        let mut session = make_cloud_session();
        session.id = "session_cloud_1".into();
        let mut picker = SessionPicker::new(vec![session.clone()]);
        picker
            .cloud_presence
            .streaming
            .insert(session.id.clone(), None);
        picker.merge_cloud_presence();
        assert!(picker.session_is_streaming(&session));
        // A local refresh must not drop the remote running state.
        picker.refresh_live_presence();
        assert!(picker.session_is_streaming(&session));
    }

    fn make_cloud_session() -> SessionInfo {
        let mut session = super::super::tests::make_session_for_presence("cloud");
        session.server_name = Some("cloud: jcode-cloud".into());
        session.server_icon = Some("☁".into());
        session
    }
}
