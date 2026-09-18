//! Blocking local Git worktree operations. Run these on a background thread in a UI.
//!
//! Creation checks out a new branch from the invoking worktree's HEAD, without
//! copying staged, unstaged, or untracked changes. Existing branches and paths
//! are never reused. Git must be available on PATH.

use anyhow::{Context, Result, bail, ensure};
use std::path::Path;
use std::process::Command;

/// A worktree reported by Git. Branches are full refs (e.g. `refs/heads/main`).
/// Bare repositories have an empty `head`. Flag-only lock/prune reasons are
/// represented by `Some(String::new())`. Paths must be valid UTF-8.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Worktree {
    pub path: String,
    pub branch: Option<String>,
    pub head: String,
    pub bare: bool,
    pub detached: bool,
    pub locked: Option<String>,
    pub prunable: Option<String>,
}

fn git(working_dir: &str, args: &[&str]) -> Result<Vec<u8>> {
    let output = Command::new("git")
        .current_dir(working_dir)
        .args(args)
        .output()
        .context("Failed to run Git")?;
    ensure!(
        output.status.success(),
        "git {} failed: {}",
        args.first().unwrap_or(&""),
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(output.stdout)
}

/// List all worktrees, including bare, detached, locked, and prunable entries.
/// Accepts the repository root or any directory within a worktree.
pub fn list(working_dir: &str) -> Result<Vec<Worktree>> {
    parse(&git(
        working_dir,
        &["worktree", "list", "--porcelain", "-z"],
    )?)
}

fn parse(bytes: &[u8]) -> Result<Vec<Worktree>> {
    let text = std::str::from_utf8(bytes).context("Git worktree output is not UTF-8")?;
    let mut entries = Vec::new();
    let mut current: Option<Worktree> = None;
    for field in text.split('\0') {
        if field.is_empty() {
            if let Some(entry) = current.take() {
                entries.push(entry);
            }
            continue;
        }
        let (key, value) = field.split_once(' ').unwrap_or((field, ""));
        if key == "worktree" {
            ensure!(current.is_none(), "Missing worktree record separator");
            current = Some(Worktree {
                path: value.to_owned(),
                branch: None,
                head: String::new(),
                bare: false,
                detached: false,
                locked: None,
                prunable: None,
            });
            continue;
        }
        let entry = current.as_mut().context("Missing worktree path")?;
        match key {
            "HEAD" => entry.head = value.to_owned(),
            "branch" => entry.branch = Some(value.to_owned()),
            "bare" => entry.bare = true,
            "detached" => entry.detached = true,
            "locked" => entry.locked = Some(value.to_owned()),
            "prunable" => entry.prunable = Some(value.to_owned()),
            _ => {} // Forward compatible with additional Git attributes.
        }
    }
    if let Some(entry) = current {
        entries.push(entry);
    }
    Ok(entries)
}

// Encode rather than replace separators, so feature/a and feature-a cannot collide.
fn safe_name(branch: &str) -> String {
    let mut name = String::new();
    for byte in branch.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_') {
            name.push(char::from(byte));
        } else {
            use std::fmt::Write;
            write!(name, "%{byte:02X}").expect("writing to String");
        }
    }
    name
}

/// Create a new branch from the caller's HEAD and check it out in a sibling
/// `<main-repo-name>-worktrees/<encoded-branch>` directory. Nested and linked
/// worktree invocations share the same destination root. No force/reset flags
/// are used. Errors never trigger destructive cleanup, so a Git failure may
/// leave a newly created branch or directory for the user to inspect.
pub fn create(working_dir: &str, branch: &str) -> Result<Worktree> {
    ensure!(
        !branch.is_empty() && !branch.starts_with('-'),
        "Invalid branch name"
    );
    let checked = git(working_dir, &["check-ref-format", "--branch", branch])?;
    // Reject Git shorthand such as @{-1}, which check-ref-format expands.
    ensure!(
        checked == format!("{branch}\n").as_bytes(),
        "Branch name must be literal"
    );
    let entries = list(working_dir)?;
    let main = Path::new(&entries.first().context("Repository has no worktrees")?.path);
    let parent = main
        .parent()
        .context("Repository has no parent directory")?;
    let name = main
        .file_name()
        .context("Repository has no directory name")?
        .to_str()
        .context("Repository name is not UTF-8")?;
    let root = parent.join(format!("{name}-worktrees"));
    if let Ok(meta) = std::fs::symlink_metadata(&root) {
        ensure!(
            meta.is_dir() && !meta.file_type().is_symlink(),
            "Worktree root is not a real directory"
        );
    }
    let path = root.join(safe_name(branch));
    match std::fs::symlink_metadata(&path) {
        Ok(_) => bail!("Worktree destination already exists: {}", path.display()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("Cannot inspect worktree destination"),
    }
    let path = path.to_str().context("Worktree path is not UTF-8")?;
    // -b, not -B: Git atomically refuses an already-existing branch.
    git(
        working_dir,
        &["worktree", "add", "-b", branch, "--", path, "HEAD"],
    )?;
    list(working_dir)?
        .into_iter()
        .find(|entry| entry.path == path)
        .context("Created worktree was not present in Git's worktree list")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn repo() -> (TempDir, String) {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("repo with spaces");
        std::fs::create_dir(&path).unwrap();
        let path = path.to_str().unwrap().to_owned();
        git(&path, &["init", "-b", "main"]).unwrap();
        // Fixture-only identity, never used for repository development commits.
        git(&path, &["config", "user.name", "SDK Test"]).unwrap();
        git(&path, &["config", "user.email", "sdk-test@example.invalid"]).unwrap();
        std::fs::write(Path::new(&path).join("tracked"), "original").unwrap();
        git(&path, &["add", "tracked"]).unwrap();
        git(&path, &["commit", "-m", "initial"]).unwrap();
        (temp, path)
    }

    #[test]
    fn parses_nul_records_without_trimming_paths_or_reasons() {
        let entries = parse(b"worktree /repo with\nnewline \0HEAD abc\0branch refs/heads/main\0locked reason\nwith newline \0\0worktree /bare\0bare\0\0worktree /detached\0HEAD def\0detached\0locked\0prunable missing path\0\0").unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].path, "/repo with\nnewline ");
        assert_eq!(entries[0].locked.as_deref(), Some("reason\nwith newline "));
        assert!(entries[1].bare);
        assert_eq!(entries[1].head, "");
        assert!(entries[2].detached);
        assert_eq!(entries[2].branch, None);
        assert_eq!(entries[2].locked.as_deref(), Some(""));
        assert_eq!(entries[2].prunable.as_deref(), Some("missing path"));
        assert!(parse(b"worktree /\xff\0\0").is_err());
    }

    #[test]
    fn creation_isolates_dirty_edits_and_nested_invocation() {
        let (temp, path) = repo();
        let before = list(&path).unwrap().remove(0);
        std::fs::write(Path::new(&path).join("tracked"), "staged").unwrap();
        git(&path, &["add", "tracked"]).unwrap();
        std::fs::write(Path::new(&path).join("tracked"), "unstaged").unwrap();
        std::fs::write(Path::new(&path).join("untracked"), "private").unwrap();
        let nested = Path::new(&path).join("nested");
        std::fs::create_dir(&nested).unwrap();
        let created = create(nested.to_str().unwrap(), "feature/one").unwrap();
        assert_eq!(created.head, before.head);
        assert_eq!(created.branch.as_deref(), Some("refs/heads/feature/one"));
        assert_eq!(
            Path::new(&created.path),
            temp.path().join("repo with spaces-worktrees/feature%2Fone")
        );
        assert_eq!(
            std::fs::read_to_string(Path::new(&created.path).join("tracked")).unwrap(),
            "original"
        );
        assert!(!Path::new(&created.path).join("untracked").exists());
        assert_eq!(
            std::fs::read_to_string(Path::new(&path).join("tracked")).unwrap(),
            "unstaged"
        );
        assert_eq!(git(&path, &["show", ":tracked"]).unwrap(), b"staged");
        git(
            &created.path,
            &["commit", "--allow-empty", "-m", "linked HEAD"],
        )
        .unwrap();
        let linked_head = git(&created.path, &["rev-parse", "HEAD"]).unwrap();
        let second = create(&created.path, "feature-two").unwrap();
        assert_eq!(format!("{}\n", second.head).as_bytes(), linked_head);
        assert_ne!(second.head, before.head);
        assert_eq!(
            Path::new(&second.path).parent(),
            Path::new(&created.path).parent()
        );
        assert_eq!(list(&path).unwrap().len(), 3);
    }

    #[test]
    fn rejects_invalid_branches_collisions_and_non_repositories() {
        let (temp, path) = repo();
        for branch in [
            "",
            "-oops",
            "--detach",
            "a b",
            "../escape",
            "a..b",
            "a\nb",
            "@{-1}",
        ] {
            assert!(create(&path, branch).is_err(), "accepted {branch:?}");
        }
        assert!(create(&path, "main").is_err());
        let created = create(&path, "new").unwrap();
        assert!(create(&path, "new").is_err());
        assert!(Path::new(&created.path).exists());
        let occupied = temp.path().join("repo with spaces-worktrees/occupied");
        std::fs::create_dir(&occupied).unwrap();
        assert!(create(&path, "occupied").is_err());
        assert!(git(&path, &["show-ref", "--verify", "refs/heads/occupied"]).is_err());
        assert!(list(temp.path().to_str().unwrap()).is_err());
        assert!(create(temp.path().to_str().unwrap(), "valid").is_err());
        assert_ne!(safe_name("feature/a"), safe_name("feature-a"));
        assert_ne!(safe_name("feature/a"), safe_name("feature%2Fa"));
    }

    #[test]
    fn lists_existing_detached_locked_prunable_and_bare_worktrees() {
        let (temp, path) = repo();
        let detached = temp.path().join("detached\nwith space");
        let detached = detached.to_str().unwrap();
        git(&path, &["worktree", "add", "--detach", detached, "HEAD"]).unwrap();
        git(
            &path,
            &["worktree", "lock", "--reason", "keep\nthis", detached],
        )
        .unwrap();
        let entry = list(&path)
            .unwrap()
            .into_iter()
            .find(|w| w.path == detached)
            .unwrap();
        assert!(entry.detached);
        assert_eq!(entry.locked.as_deref(), Some("keep\nthis"));
        git(&path, &["worktree", "unlock", detached]).unwrap();
        std::fs::remove_dir_all(detached).unwrap();
        assert!(
            list(&path)
                .unwrap()
                .iter()
                .any(|w| w.path == detached && w.prunable.is_some())
        );
        let bare = temp.path().join("bare.git");
        git(&path, &["clone", "--bare", &path, bare.to_str().unwrap()]).unwrap();
        let entries = list(bare.to_str().unwrap()).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].bare);
        let created = create(bare.to_str().unwrap(), "from-bare").unwrap();
        assert_eq!(created.branch.as_deref(), Some("refs/heads/from-bare"));
    }

    #[test]
    fn unborn_head_does_not_create_a_branch() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().to_str().unwrap();
        git(path, &["init", "-b", "main"]).unwrap();
        assert!(create(path, "new").is_err());
        assert!(git(path, &["show-ref", "--verify", "refs/heads/new"]).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn refuses_symlink_destination_root() {
        let (temp, path) = repo();
        let elsewhere = temp.path().join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, temp.path().join("repo with spaces-worktrees"))
            .unwrap();
        assert!(create(&path, "new").is_err());
        assert_eq!(std::fs::read_dir(elsewhere).unwrap().count(), 0);
    }
}
