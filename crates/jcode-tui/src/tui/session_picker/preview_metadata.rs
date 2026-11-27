//! Selected-workspace facts. All filesystem and SSH work runs in the preview loader.
use crate::tui::info_widget::InfoWidgetData;

#[derive(Clone)]
pub(super) struct WorkspaceTarget {
    pub dir: Option<String>,
    pub host: Option<String>,
    /// A cloud row without an authoritative lease must not probe a local path.
    pub unavailable: bool,
}

impl WorkspaceTarget {
    pub fn for_identity(session_id: Option<&str>, dir: Option<String>, is_cloud: bool) -> Self {
        let host = session_id.and_then(|session_id| {
            crate::storage::read_session_lease(session_id)
                .and_then(|lease| lease.away_host)
                .filter(|host| !host.trim().is_empty())
        });
        Self {
            dir,
            unavailable: is_cloud && host.is_none(),
            host,
        }
    }
}

pub(super) fn hydrate_workspace(data: &mut InfoWidgetData, target: &WorkspaceTarget) {
    if target.unavailable {
        return;
    }
    // The persisted directory is a fallback only. The list may have a newer
    // directory after a session's chdir, so prefer the selected index identity.
    let Some(dir) = target.dir.as_deref().or(data.working_dir.as_deref()) else {
        return;
    };
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return;
    };
    let config = super::preview_result(crate::storage::jcode_dir(), "SSH config directory")
        .map(|dir| dir.join("cloud/ssh_config"));
    let config = config.as_deref().filter(|path| path.is_file());
    if let crate::tui::cloud_git::RepoState::Ready(info) = runtime.block_on(
        crate::tui::cloud_git::probe_workspace(target.host.as_deref(), config, dir),
    ) {
        data.git_info = Some(info);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_workspace_git_uses_selected_directory_not_process_cwd() {
        let repo = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            assert!(
                std::process::Command::new("git")
                    .args(args)
                    .current_dir(repo.path())
                    .output()
                    .unwrap()
                    .status
                    .success()
            );
        };
        run(&["init", "-q"]);
        run(&["symbolic-ref", "HEAD", "refs/heads/selected-preview"]);
        std::fs::write(repo.path().join("selected-file"), "pending change").unwrap();
        let mut data = InfoWidgetData::default();
        hydrate_workspace(
            &mut data,
            &WorkspaceTarget {
                dir: Some(repo.path().to_string_lossy().into_owned()),
                host: None,
                unavailable: false,
            },
        );
        assert!(data.has_data_for(crate::tui::info_widget::WidgetKind::GitStatus));
        let footer = crate::tui::ui::input_ui::preview_overscroll_line(&data, 160).to_string();
        assert!(footer.contains("selected-preview"), "{footer}");
        let git = data.git_info.unwrap();
        assert_eq!(git.branch, "selected-preview");
        assert_eq!(git.untracked, 1);
        assert_eq!(git.repo_root.as_deref(), Some(repo.path()));
    }

    #[test]
    fn preview_workspace_missing_or_unknown_remote_never_falls_back_to_cwd() {
        for target in [
            WorkspaceTarget {
                dir: None,
                host: None,
                unavailable: false,
            },
            WorkspaceTarget {
                dir: Some("/".into()),
                host: None,
                unavailable: true,
            },
            WorkspaceTarget {
                dir: Some("/".into()),
                host: Some("-invalid".into()),
                unavailable: false,
            },
        ] {
            let mut data = InfoWidgetData::default();
            hydrate_workspace(&mut data, &target);
            assert!(data.git_info.is_none());
        }
    }
}
