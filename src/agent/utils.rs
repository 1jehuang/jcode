use crate::session::GitState;
use std::path::Path;
use std::process::Command;

use super::Agent;

pub(super) fn trace_enabled() -> bool {
    match std::env::var("JCODE_TRACE") {
        Ok(value) => {
            let value = value.trim();
            !value.is_empty() && value != "0" && value.to_lowercase() != "false"
        }
        Err(_) => false,
    }
}

pub(super) fn git_state_for_dir(dir: &Path) -> Option<GitState> {
    let root = git_output(dir, &["rev-parse", "--show-toplevel"])?;
    let head = git_output(dir, &["rev-parse", "HEAD"]);
    let branch = git_output(dir, &["rev-parse", "--abbrev-ref", "HEAD"]);
    let dirty = git_output(dir, &["status", "--porcelain"]).map(|out| !out.is_empty());

    Some(GitState {
        root,
        head,
        branch,
        dirty,
    })
}

fn git_output(dir: &Path, args: &[&str]) -> Option<String> {
    let mut cmd = Command::new("git");
    cmd.args(args).current_dir(dir);
    crate::platform::suppress_child_console(&mut cmd);
    let output = cmd.output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

impl Agent {
    pub(super) fn update_generated_image_side_panel(
        &self,
        id: &str,
        path: &str,
        metadata_path: Option<&str>,
        output_format: &str,
        revised_prompt: Option<&str>,
    ) -> Option<crate::side_panel::SidePanelSnapshot> {
        match crate::tui::write_generated_image_side_panel_page(
            &self.session.id,
            id,
            path,
            metadata_path,
            output_format,
            revised_prompt,
        ) {
            Ok(snapshot) => {
                crate::bus::Bus::global().publish(crate::bus::BusEvent::SidePanelUpdated(
                    crate::bus::SidePanelUpdated {
                        session_id: self.session.id.clone(),
                        snapshot: snapshot.clone(),
                    },
                ));
                Some(snapshot)
            }
            Err(err) => {
                crate::logging::warn(&format!(
                    "Failed to write generated image side panel page: {}",
                    err
                ));
                None
            }
        }
    }
}
