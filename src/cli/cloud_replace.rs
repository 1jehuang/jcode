//! Repository-only replacement deliberately bypasses all session/auth transfer paths.
use super::*;

pub(super) fn run(target: &Target, json: bool) -> Result<()> {
    let host = prepare_host(target)?;
    target
        .run_remote(&host, &["cloud", "replace-receive", "--help"], None)
        .context("cloud host needs a jcode version supporting cloud replace")?;
    let stage = tempfile::tempdir()?;
    let id = format!("replace-{}", uuid::Uuid::new_v4());
    let mut repo = prepare_repo_snapshot(&std::env::current_dir()?, &id, stage.path())?
        .context("cloud replace requires a Git repository with an initial commit")?;
    // No ignored-file archive, origin credentials, account files or session state.
    repo.ignored_files.clear();
    repo.origin_url = None;
    create_repo_bundle(&repo, &id, stage.path(), None)?;
    std::fs::write(stage.path().join("repo.json"), serde_json::to_vec(&repo)?)?;
    std::fs::write(stage.path().join("id"), &id)?;
    let archive = stage.path().join("replace.tar.gz");
    tar_dir(stage.path(), &archive, &["repo.json", "id", "repo.bundle"])?;
    // Not retried: a lost response must not initiate another replacement.
    let output = target.run_remote(&host, &["cloud", "replace-receive"], Some(&archive))?;
    let report: serde_json::Value = serde_json::from_slice(&output).context(
        "replacement response lost or invalid; inspect cloud checkout and backups before retrying",
    )?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "Replaced {} on `{host}`. Backup: {}. No session or credentials moved.",
            repo.root,
            report["backup_path"]
                .as_str()
                .unwrap_or("none (checkout was absent)")
        );
    }
    Ok(())
}

fn same_repository(path: &Path, root: &Path) -> bool {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    path.starts_with(&root) || root.starts_with(&path)
}

fn reject_lease(root: &Path, lease: &storage::SessionLease) -> Result<()> {
    if lease.is_here()
        && lease
            .repo_root
            .as_deref()
            .is_none_or(|p| same_repository(Path::new(p), root))
    {
        bail!(
            "repository is owned by cloud session {}; return that session before replacing",
            lease.session_id
        );
    }
    Ok(())
}

fn reject_active_session(root: &Path, id: &str, working_dir: Option<&str>) -> Result<()> {
    if working_dir.is_none_or(|p| same_repository(Path::new(p), root)) {
        bail!("repository may be in use by active cloud session {id}; stop it before replacing");
    }
    Ok(())
}

fn reject_owners(root: &Path) -> Result<()> {
    if let Some(dir) = storage::session_leases_dir() {
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => Some(entries),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error).context("cannot inspect cloud session leases"),
        };
        for entry in entries.into_iter().flatten() {
            let entry = entry?;
            let path = entry.path();
            let Some(id) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if path.extension().is_none_or(|e| e != "json") {
                continue;
            }
            let lease = storage::read_session_lease(id)
                .with_context(|| format!("cannot inspect cloud session lease {id}"))?;
            reject_lease(root, &lease)?;
        }
    }
    for presence in storage::session_presence() {
        let id = presence.session_id;
        let sess = session::Session::load(&id).with_context(|| {
            format!("cannot verify repository for active session {id}; refusing replacement")
        })?;
        reject_active_session(root, &id, sess.working_dir.as_deref())?;
    }
    Ok(())
}

fn install(repo: &RepoManifest, bundle: &Path, id: &str) -> Result<Option<PathBuf>> {
    let root = Path::new(&repo.root);
    if !root.is_absolute()
        || root.parent().is_none()
        || root.file_name().is_none()
        || root
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        bail!("invalid replacement repository path");
    }
    reject_owners(root)?;
    let parent = root.parent().unwrap();
    std::fs::create_dir_all(parent)?;
    // Keep the lock inode, even after unlocking, so concurrent callers cannot
    // lock different inodes. This lock also survives a receiver crash safely.
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(parent.join(format!(
            ".{}.jcode-replace.lock",
            root.file_name().unwrap().to_string_lossy()
        )))?;
    lock.try_lock()
        .context("another replacement is already running for this checkout")?;
    let prepared = tempfile::Builder::new()
        .prefix(".jcode-replace-")
        .tempdir_in(parent)?;
    let mut staged = repo.clone();
    staged.root = prepared
        .path()
        .join("checkout")
        .to_string_lossy()
        .into_owned();
    restore_repo(
        &staged,
        bundle,
        &prepared.path().join("no-ignored-files"),
        id,
    )?;
    reject_owners(root)?;
    let backup = if root.symlink_metadata().is_ok() {
        let backup = parent.join(format!(
            "{}.jcode-backup-{}",
            root.file_name().unwrap().to_string_lossy(),
            uuid::Uuid::new_v4()
        ));
        std::fs::rename(root, &backup).context("preserve existing cloud checkout")?;
        Some(backup)
    } else {
        None
    };
    if let Err(error) = std::fs::rename(&staged.root, root) {
        if let Some(backup) = &backup {
            std::fs::rename(backup, root).with_context(|| {
                format!(
                    "replacement failed ({error}); original remains at {}",
                    backup.display()
                )
            })?;
        }
        return Err(error.into());
    }
    Ok(backup)
}

pub(super) fn receive() -> Result<()> {
    let stage = tempfile::tempdir()?;
    let archive = stage.path().join("incoming.tar.gz");
    let mut file = std::fs::File::create(&archive)?;
    std::io::copy(&mut std::io::stdin(), &mut file)?;
    untar(&archive, stage.path())?;
    let repo: RepoManifest = read_json(&stage.path().join("repo.json"))?;
    let id = std::fs::read_to_string(stage.path().join("id"))?;
    if !id.starts_with("replace-") || uuid::Uuid::parse_str(&id[8..]).is_err() {
        bail!("invalid replacement id");
    }
    if !repo.ignored_files.is_empty() || repo.origin_url.is_some() {
        bail!("repository-only replacement cannot import ignored files or origin credentials");
    }
    let backup = install(&repo, &stage.path().join("repo.bundle"), &id)?;
    println!(
        "{}",
        serde_json::json!({"repo_root": repo.root, "backup_path": backup})
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Ownership discovery uses process-global storage. Run filesystem cases in
    // a child with a private home rather than changing env in parallel tests.
    fn isolated(name: &str) -> bool {
        if std::env::var("JCODE_REPLACE_TEST_CHILD").as_deref() == Ok(name) {
            return false;
        }
        let home = tempfile::tempdir().unwrap();
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                &format!("cli::cloud_move::replace::tests::{name}"),
                "--nocapture",
            ])
            .env("JCODE_HOME", home.path())
            .env("JCODE_REPLACE_TEST_CHILD", name)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        true
    }

    #[test]
    fn replacement_rejects_other_session_ownership() {
        let mut lease = storage::SessionLease {
            session_id: "another-session".into(),
            epoch: 3,
            away_host: None,
            repo_root: Some("/repository".into()),
            last_sync_snapshot: None,
            last_sync_head: None,
            updated_at: String::new(),
        };
        let root = Path::new("/repository");
        assert!(
            reject_lease(root, &lease)
                .unwrap_err()
                .to_string()
                .contains("another-session")
        );
        lease.away_host = Some("returned".into());
        assert!(reject_lease(root, &lease).is_ok());
        lease.away_host = None;
        lease.repo_root = Some("/unrelated".into());
        assert!(reject_lease(root, &lease).is_ok());
        lease.repo_root = None;
        assert!(reject_lease(root, &lease).is_err());
        assert!(
            reject_active_session(root, "different-session", Some("/repository/nested")).is_err()
        );
        assert!(reject_active_session(root, "different-session", Some("/unrelated")).is_ok());
        assert!(reject_active_session(root, "unknown-session", None).is_err());
    }

    #[test]
    fn replacement_refuses_live_owners_without_touching_checkout() {
        if isolated("replacement_refuses_live_owners_without_touching_checkout") {
            return;
        }
        let (_source, stage, mut repo) = source();
        let destination = tempfile::tempdir().unwrap();
        let root = destination.path().join("repo");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("precious"), "keep").unwrap();
        repo.root = root.to_string_lossy().into_owned();
        let lease = storage::SessionLease {
            session_id: "different-session".into(),
            epoch: 1,
            away_host: None,
            repo_root: Some(repo.root.clone()),
            last_sync_snapshot: None,
            last_sync_head: None,
            updated_at: String::new(),
        };
        storage::write_session_lease(&lease).unwrap();
        assert!(
            install(&repo, &stage.path().join("repo.bundle"), "replacement")
                .unwrap_err()
                .to_string()
                .contains("different-session")
        );
        storage::remove_session_lease("different-session");
        let mut sess = session::Session::create_with_id("live-other-session".into(), None, None);
        sess.working_dir = Some(root.join("nested").to_string_lossy().into_owned());
        sess.save().unwrap();
        storage::register_active_pid(&sess.id, std::process::id());
        assert!(
            install(&repo, &stage.path().join("repo.bundle"), "replacement")
                .unwrap_err()
                .to_string()
                .contains("live-other-session")
        );
        assert_eq!(
            std::fs::read_to_string(root.join("precious")).unwrap(),
            "keep"
        );
        assert!(
            !destination.path().read_dir().unwrap().any(|e| e
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains("backup"))
        );
    }

    #[test]
    fn replacement_cli_has_no_session_or_auth_arguments() {
        use crate::cli::args::{Args, CloudCommand, Command};
        use clap::Parser;
        let args =
            Args::try_parse_from(["jcode", "cloud", "replace", "--host", "test-host", "--json"])
                .unwrap();
        assert!(matches!(
            args.command,
            Some(Command::Cloud(CloudCommand::Replace { json: true, .. }))
        ));
        assert!(Args::try_parse_from(["jcode", "cloud", "replace", "--session", "live"]).is_err());
        assert!(Args::try_parse_from(["jcode", "cloud", "replace", "--allow-active"]).is_err());
    }

    fn source() -> (tempfile::TempDir, tempfile::TempDir, RepoManifest) {
        let (root, stage, _) = super::super::repo_tests::repository();
        std::fs::write(root.path().join("file.txt"), "local changes\n").unwrap();
        std::fs::write(root.path().join("untracked.txt"), "new local file\n").unwrap();
        let repo = snapshot_repo(root.path(), "replacement", stage.path())
            .unwrap()
            .unwrap();
        (root, stage, repo)
    }

    #[test]
    fn replacement_preserves_entire_previous_checkout() {
        if isolated("replacement_preserves_entire_previous_checkout") {
            return;
        }
        let (_source, stage, mut repo) = source();
        let destination = tempfile::tempdir().unwrap();
        let root = destination.path().join("repo");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("cloud-only"), "precious cloud work").unwrap();
        std::fs::write(root.join(".env"), "cloud secret").unwrap();
        repo.root = root.to_string_lossy().into_owned();
        let backup = install(&repo, &stage.path().join("repo.bundle"), "replacement")
            .unwrap()
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(backup.join("cloud-only")).unwrap(),
            "precious cloud work"
        );
        assert_eq!(
            std::fs::read_to_string(backup.join(".env")).unwrap(),
            "cloud secret"
        );
        assert!(!root.join("cloud-only").exists());
        assert!(!root.join(".env").exists());
        assert_eq!(
            std::fs::read_to_string(root.join("file.txt")).unwrap(),
            "local changes\n"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("untracked.txt")).unwrap(),
            "new local file\n"
        );
        assert_eq!(git(&root, &["rev-parse", "HEAD"]).unwrap(), repo.head);
        assert!(!git(&root, &["status", "--porcelain"]).unwrap().is_empty());
    }

    #[test]
    fn replacement_creates_absent_checkout() {
        if isolated("replacement_creates_absent_checkout") {
            return;
        }
        let (_source, stage, mut repo) = source();
        let destination = tempfile::tempdir().unwrap();
        let root = destination.path().join("missing-parent/repo");
        repo.root = root.to_string_lossy().into_owned();
        assert!(
            install(&repo, &stage.path().join("repo.bundle"), "replacement")
                .unwrap()
                .is_none()
        );
        assert!(root.join(".git").is_dir());
        assert!(root.join("untracked.txt").is_file());
    }

    #[test]
    fn invalid_bundle_never_moves_existing_checkout() {
        if isolated("invalid_bundle_never_moves_existing_checkout") {
            return;
        }
        let (_source, stage, mut repo) = source();
        let destination = tempfile::tempdir().unwrap();
        let root = destination.path().join("repo");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("precious"), "keep").unwrap();
        repo.root = root.to_string_lossy().into_owned();
        std::fs::write(stage.path().join("bad.bundle"), "not a bundle").unwrap();
        assert!(install(&repo, &stage.path().join("bad.bundle"), "replacement").is_err());
        assert_eq!(
            std::fs::read_to_string(root.join("precious")).unwrap(),
            "keep"
        );
        assert!(
            !destination.path().read_dir().unwrap().any(|e| e
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains("backup"))
        );
    }

    #[test]
    fn repository_ownership_includes_subdirectories_and_ancestors() {
        assert!(same_repository(
            Path::new("/repo/subdir"),
            Path::new("/repo")
        ));
        assert!(same_repository(
            Path::new("/repo"),
            Path::new("/repo/subdir")
        ));
        assert!(!same_repository(
            Path::new("/repo-other"),
            Path::new("/repo")
        ));
    }
}
