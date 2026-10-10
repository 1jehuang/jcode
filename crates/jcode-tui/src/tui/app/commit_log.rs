//! Commit log overlay (default Alt+G): a scrollable `git log` of the project
//! the session is working in.

use super::*;

/// Maximum number of commits loaded into the overlay.
const COMMIT_LOG_LIMIT: usize = 500;

/// One commit row in the overlay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommitLogEntry {
    pub hash: String,
    pub date: String,
    pub author: String,
    pub refs: String,
    pub subject: String,
}

/// State of the open commit log overlay.
#[derive(Debug, Clone, Default)]
pub(crate) struct CommitLogOverlay {
    pub scroll: usize,
    /// Repository label shown in the title (repo root plus branch).
    pub title: String,
    pub entries: Vec<CommitLogEntry>,
    /// Error text shown instead of entries (e.g. not a git repository).
    pub error: Option<String>,
}

/// Parse `git log --format=%h%x1f%ad%x1f%an%x1f%D%x1f%s` output.
pub(crate) fn parse_commit_log(text: &str) -> Vec<CommitLogEntry> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| {
            let mut fields = line.splitn(5, '\x1f');
            Some(CommitLogEntry {
                hash: fields.next()?.trim().to_string(),
                date: fields.next().unwrap_or("").trim().to_string(),
                author: fields.next().unwrap_or("").trim().to_string(),
                refs: fields.next().unwrap_or("").trim().to_string(),
                subject: fields.next().unwrap_or("").trim().to_string(),
            })
        })
        .collect()
}

/// Load the commit log for `dir` (or the process working directory).
pub(crate) fn load_commit_log(dir: Option<&std::path::Path>) -> CommitLogOverlay {
    let git = |args: &[&str]| {
        let mut cmd = std::process::Command::new("git");
        if let Some(dir) = dir {
            cmd.current_dir(dir);
        }
        cmd.args(args).output()
    };

    let root = git(&["rev-parse", "--show-toplevel"])
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    let Some(root) = root else {
        let where_ = dir
            .map(|d| d.display().to_string())
            .unwrap_or_else(|| "the current directory".to_string());
        return CommitLogOverlay {
            title: "Commit log".to_string(),
            error: Some(format!("Not a git repository: {where_}")),
            ..Default::default()
        };
    };

    let branch = git(&["rev-parse", "--abbrev-ref", "HEAD"])
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    let title = if branch.is_empty() {
        root.clone()
    } else {
        format!("{root} ({branch})")
    };

    let limit = format!("-n{COMMIT_LOG_LIMIT}");
    match git(&[
        "log",
        &limit,
        "--no-color",
        "--date=format:%Y-%m-%d %H:%M",
        "--format=%h%x1f%ad%x1f%an%x1f%D%x1f%s",
    ]) {
        Ok(output) if output.status.success() => CommitLogOverlay {
            title,
            entries: parse_commit_log(&String::from_utf8_lossy(&output.stdout)),
            ..Default::default()
        },
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            CommitLogOverlay {
                title,
                error: Some(if stderr.is_empty() {
                    "git log failed".to_string()
                } else {
                    stderr
                }),
                ..Default::default()
            }
        }
        Err(err) => CommitLogOverlay {
            title,
            error: Some(format!("Failed to run git: {err}")),
            ..Default::default()
        },
    }
}

impl App {
    /// Open the commit log overlay for the session's project, or close it if open.
    pub(super) fn toggle_commit_log_overlay(&mut self) {
        if self.commit_log_overlay.is_some() {
            self.commit_log_overlay = None;
            return;
        }
        let dir = self
            .session
            .working_dir
            .as_deref()
            .map(std::path::PathBuf::from)
            .filter(|p| p.is_dir());
        self.commit_log_overlay = Some(load_commit_log(dir.as_deref()));
    }

    pub(super) fn handle_commit_log_key(&mut self, code: KeyCode) -> Result<()> {
        let Some(overlay) = self.commit_log_overlay.as_mut() else {
            return Ok(());
        };
        let scroll = overlay.scroll;
        let max = overlay.entries.len().saturating_sub(1);
        match code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.commit_log_overlay = None;
                return Ok(());
            }
            KeyCode::Down | KeyCode::Char('j') => overlay.scroll = scroll.saturating_add(1),
            KeyCode::Up | KeyCode::Char('k') => overlay.scroll = scroll.saturating_sub(1),
            KeyCode::PageDown | KeyCode::Char(' ') | KeyCode::Char('d') => {
                overlay.scroll = scroll.saturating_add(20)
            }
            KeyCode::PageUp | KeyCode::Char('u') => overlay.scroll = scroll.saturating_sub(20),
            KeyCode::Home | KeyCode::Char('g') => overlay.scroll = 0,
            KeyCode::End | KeyCode::Char('G') => overlay.scroll = max,
            KeyCode::Char('c') | KeyCode::Char('y') => {
                let hash = overlay
                    .entries
                    .get(scroll.min(max))
                    .map(|entry| entry.hash.clone());
                if let Some(hash) = hash {
                    if super::helpers::copy_to_clipboard(&hash) {
                        self.set_status_notice(format!("Copied commit {hash}"));
                    } else {
                        self.set_status_notice("Failed to copy commit hash");
                    }
                }
                return Ok(());
            }
            _ => {}
        }
        if let Some(overlay) = self.commit_log_overlay.as_mut() {
            overlay.scroll = overlay.scroll.min(max);
        }
        Ok(())
    }

    pub(super) fn scroll_commit_log_overlay(&mut self, direction: i16) -> bool {
        let Some(overlay) = self.commit_log_overlay.as_mut() else {
            return false;
        };
        let max = overlay.entries.len().saturating_sub(1);
        overlay.scroll = if direction < 0 {
            overlay.scroll.saturating_sub(1)
        } else {
            overlay.scroll.saturating_add(1).min(max)
        };
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_commit_log_records() {
        let text = "abc1234\x1f2026-10-09 22:00\x1fJeremy\x1fHEAD -> master, origin/master\x1ftui: add thing\n\
                    def5678\x1f2026-10-08 10:00\x1fAlice\x1f\x1ffix: subject with \x1f separator\n";
        let entries = parse_commit_log(text);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].hash, "abc1234");
        assert_eq!(entries[0].refs, "HEAD -> master, origin/master");
        assert_eq!(entries[0].subject, "tui: add thing");
        assert_eq!(entries[1].refs, "");
        assert_eq!(entries[1].subject, "fix: subject with \x1f separator");
    }

    #[test]
    fn non_repo_reports_error() {
        let dir = tempfile::tempdir().unwrap();
        let overlay = load_commit_log(Some(dir.path()));
        assert!(overlay.entries.is_empty());
        assert!(overlay.error.is_some());
    }
}
