use crate::session::GitState;
use std::path::Path;
use std::process::Command;

pub(super) fn trace_enabled() -> bool {
    match std::env::var("KCODE_TRACE") {
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
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}
