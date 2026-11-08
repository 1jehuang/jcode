//! Repository replacement bypasses session/provider-auth migration. Explicit
//! saved environment requirements are replayed separately after the swap.
use super::*;

pub(super) fn run(target: &Target, yes: bool, dry_run: bool, json: bool) -> Result<()> {
    if !yes && !dry_run {
        bail!(
            "cloud replace requires --yes to replace the cloud checkout; use --dry-run to preview first"
        );
    }
    let host = target.resolve_host()?;
    target.preflight(&host)?;
    target
        .run_remote(&host, &["cloud", "replace-receive", "--help"], None)
        .context("cloud host needs a jcode version supporting cloud replace")?;
    let stage = tempfile::tempdir()?;
    let id = format!("replace-{}", uuid::Uuid::new_v4());
    let mut repo = prepare_repo_snapshot(&std::env::current_dir()?, &id, stage.path())?
        .context("cloud replace requires a Git repository with an initial commit")?;
    // The repository archive never includes ignored files, origin credentials,
    // account files or session state. Explicit environment replay is separate.
    repo.ignored_files.clear();
    repo.origin_url = None;
    create_repo_bundle(&repo, &id, stage.path(), None)?;
    std::fs::write(stage.path().join("repo.json"), serde_json::to_vec(&repo)?)?;
    std::fs::write(stage.path().join("id"), &id)?;
    let archive = stage.path().join("replace.tar.gz");
    tar_dir(stage.path(), &archive, &["repo.json", "id", "repo.bundle"])?;
    // Not retried: a lost response must not initiate another replacement.
    let mut args = vec!["cloud", "replace-receive"];
    if dry_run {
        args.push("--dry-run");
    }
    let output = target.run_remote(&host, &args, Some(&archive))?;
    let mut report: serde_json::Value = serde_json::from_slice(&output).context(
        "replacement response lost or invalid; inspect cloud checkout and backups before retrying",
    )?;
    if !report.is_object()
        || report["dry_run"].as_bool() != Some(dry_run)
        || report["repo_root"].as_str() != Some(repo.root.as_str())
    {
        bail!("invalid replacement response; inspect cloud checkout and backups before retrying");
    }
    report["host"] = serde_json::Value::String(host.clone());
    if !dry_run {
        let synced = match environment::sync_saved(Path::new(&repo.root), target) {
            Ok(()) => true,
            Err(_) => {
                // Do not format private environment failures or retry the replacement.
                step(
                    "environment",
                    "repository replaced, but saved environment requirements need attention; use cloud env push to retry",
                );
                false
            }
        };
        report["environment_synced"] = serde_json::Value::Bool(synced);
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else if dry_run {
        println!(
            "Dry run OK: {} can be replaced on `{host}`. No cloud working files changed.",
            repo.root
        );
    } else {
        println!(
            "Replaced {} on `{host}`. Backup: {}. No session or provider login moved. Explicit saved environment requirements were synchronized where available.",
            repo.root,
            report["backup_path"]
                .as_str()
                .unwrap_or("none (checkout was absent)")
        );
    }
    Ok(())
}

fn same_repository(path: &Path, root: &Path) -> Result<bool> {
    let path = resolved_path(path)?;
    let root = resolved_path(root)?;
    Ok(path.starts_with(&root) || root.starts_with(&path))
}

fn reject_lease(root: &Path, lease: &storage::SessionLease) -> Result<()> {
    if (lease.is_here() || lease.away_host.as_deref() == Some("return-pending"))
        && match lease.repo_root.as_deref() {
            Some(path) => same_repository(Path::new(path), root)?,
            None => true,
        }
    {
        bail!(
            "repository is owned by cloud session {}; inactivity does not release its unreturned work. On the source machine run `jcode cloud return --session {} --refs-only`, then retry replacement. This preserves cloud work as Git refs without changing your local checkout and permanently fences the old cloud owner. If return was interrupted, rerun the same command to finish recovery; do not delete the lease",
            lease.session_id,
            lease.session_id
        );
    }
    Ok(())
}

fn reject_active_session(root: &Path, id: &str, working_dir: Option<&str>) -> Result<()> {
    if match working_dir {
        Some(path) => same_repository(Path::new(path), root)?,
        None => true,
    } {
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

fn resolved_path(path: &Path) -> Result<PathBuf> {
    match path.canonicalize() {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let parent = path.parent().context("repository path has no parent")?;
            let name = path
                .file_name()
                .context("repository path has no filename")?;
            Ok(resolved_path(parent)?.join(name))
        }
        Err(error) => Err(error).context("cannot resolve replacement destination"),
    }
}

#[cfg(test)]
mod path_error_tests {
    use super::*;

    #[test]
    #[cfg(unix)]
    fn repository_comparison_propagates_symlink_loops() {
        let temp = tempfile::tempdir().unwrap();
        let loop_path = temp.path().join("loop");
        std::os::unix::fs::symlink("loop", &loop_path).unwrap();
        assert!(same_repository(&loop_path, temp.path()).is_err());
        assert!(reject_active_session(temp.path(), "active", loop_path.to_str()).is_err());
    }

    #[test]
    fn repository_comparison_resolves_expected_missing_paths() {
        let temp = tempfile::tempdir().unwrap();
        assert!(same_repository(&temp.path().join("absent/child"), temp.path()).unwrap());
    }
}

fn validate_destination(root: &Path) -> Result<()> {
    if !root.is_absolute()
        || root.parent().is_none()
        || root.file_name().is_none()
        || root
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        bail!("invalid replacement repository path");
    }
    match root.symlink_metadata() {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                bail!("replacement destination must be a directory, not a symlink or file");
            }
            match root.join(".git").symlink_metadata() {
                Ok(metadata) if !metadata.is_dir() || metadata.file_type().is_symlink() => {
                    bail!(
                        "cannot replace a linked worktree or repository with external Git metadata"
                    );
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error).context("cannot inspect repository metadata"),
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("cannot inspect replacement destination"),
    }
    let resolved = resolved_path(root)?;
    for protected in [dirs::home_dir(), Some(storage::jcode_dir()?)]
        .into_iter()
        .flatten()
    {
        if resolved_path(&protected)?.starts_with(&resolved) {
            bail!("cannot replace a home or Jcode state directory, or one of its ancestors");
        }
    }
    Ok(())
}

fn install(repo: &RepoManifest, bundle: &Path, id: &str) -> Result<Option<PathBuf>> {
    let root = Path::new(&repo.root);
    validate_destination(root)?;
    reject_owners(root)?;
    let parent = root.parent().context("repository path has no parent")?;
    let filename = root
        .file_name()
        .context("repository path has no filename")?;
    std::fs::create_dir_all(parent)?;
    // Keep the lock inode, even after unlocking, so concurrent callers cannot
    // lock different inodes. This lock also survives a receiver crash safely.
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(parent.join(format!(
            ".{}.jcode-replace.lock",
            filename.to_string_lossy()
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
    validate_destination(root)?;
    reject_owners(root)?;
    let exists = match root.symlink_metadata() {
        Ok(_) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(error).context("cannot inspect checkout before backup"),
    };
    let backup = if exists {
        let backup = parent.join(format!(
            "{}.jcode-backup-{}",
            filename.to_string_lossy(),
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

pub(super) fn receive(dry_run: bool) -> Result<()> {
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
    let backup = if dry_run {
        validate_destination(Path::new(&repo.root))?;
        reject_owners(Path::new(&repo.root))?;
        let mut preview = repo.clone();
        preview.root = stage.path().join("preview").to_string_lossy().into_owned();
        restore_repo(
            &preview,
            &stage.path().join("repo.bundle"),
            &stage.path().join("no-ignored-files"),
            &id,
        )?;
        None
    } else {
        install(&repo, &stage.path().join("repo.bundle"), &id)?
    };
    println!(
        "{}",
        serde_json::json!({"repo_root": repo.root, "backup_path": backup, "dry_run": dry_run})
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacement_requires_confirmation_before_contacting_cloud() {
        let error = run(&Target::default(), false, false, false).unwrap_err();
        assert!(error.to_string().contains("--yes"));
    }

    #[test]
    fn replacement_rejects_protected_and_linked_paths() {
        assert!(validate_destination(Path::new("/")).is_err());
        assert!(validate_destination(Path::new("relative/repo")).is_err());
        assert!(validate_destination(Path::new("/tmp/../repo")).is_err());
        assert!(validate_destination(&storage::jcode_dir().unwrap()).is_err());
        if let Some(home) = dirs::home_dir() {
            assert!(validate_destination(&home).is_err());
        }
        let temp = tempfile::tempdir().unwrap();
        let checkout = temp.path().join("checkout");
        std::fs::create_dir(&checkout).unwrap();
        std::fs::write(checkout.join(".git"), "gitdir: elsewhere").unwrap();
        assert!(validate_destination(&checkout).is_err());
        #[cfg(unix)]
        {
            let link = temp.path().join("symlink");
            std::os::unix::fs::symlink(&checkout, &link).unwrap();
            assert!(validate_destination(&link).is_err());
        }
    }

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
        // Historical returns have no completion receipt and must remain usable.
        assert!(reject_lease(root, &lease).is_ok());
        lease.away_host = Some("return-pending".into());
        let error = reject_lease(root, &lease).unwrap_err().to_string();
        assert!(error.contains("jcode cloud return --session another-session --refs-only"));
        assert!(error.contains("do not delete the lease"));
        lease.away_host = Some("returned-complete".into());
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
    fn durable_return_replays_original_work_after_replacement() {
        if isolated("durable_return_replays_original_work_after_replacement") {
            return;
        }
        let (cloud, _stage, _) = super::super::repo_tests::repository();
        std::fs::write(cloud.path().join("cloud-only"), "unreturned work").unwrap();
        let id = "recover-session";
        let mut sess = session::Session::create_with_id(id.into(), None, None);
        sess.working_dir = Some(cloud.path().to_string_lossy().into_owned());
        sess.migration_epoch = 1;
        sess.save_prepared().unwrap();
        storage::write_session_lease(&storage::SessionLease {
            session_id: id.into(),
            epoch: 1,
            away_host: None,
            repo_root: sess.working_dir.clone(),
            last_sync_snapshot: None,
            last_sync_head: None,
            updated_at: String::new(),
        })
        .unwrap();
        let original = export_return_archive(id, 2, true).unwrap();
        let lease = storage::read_session_lease(id).unwrap();
        assert_eq!(lease.away_host.as_deref(), Some("return-pending"));
        assert!(reject_lease(cloud.path(), &lease).is_err());
        assert!(complete_return(id, 1).is_err());
        complete_return(id, 2).unwrap();
        complete_return(id, 2).unwrap(); // Lost completion response is retryable.
        let lease = storage::read_session_lease(id).unwrap();
        assert!(reject_lease(cloud.path(), &lease).is_ok());
        let (_source, stage, mut repo) = source();
        repo.root = cloud.path().to_string_lossy().into_owned();
        let backup = install(&repo, &stage.path().join("repo.bundle"), "replacement")
            .unwrap()
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(backup.join("cloud-only")).unwrap(),
            "unreturned work"
        );
        assert!(!cloud.path().join("cloud-only").exists());
        assert_eq!(export_return_archive(id, 2, true).unwrap(), original);
        let archive = tempfile::tempdir().unwrap();
        let tar = archive.path().join("return.tar");
        std::fs::write(&tar, &original).unwrap();
        untar(&tar, archive.path()).unwrap();
        let manifest: MoveManifest = read_json(&archive.path().join("manifest.json")).unwrap();
        let snapshot = manifest.repo.unwrap().snapshot;
        git(
            cloud.path(),
            &[
                "fetch",
                archive.path().join("repo.bundle").to_str().unwrap(),
                &snapshot,
            ],
        )
        .unwrap();
        assert_eq!(
            git(cloud.path(), &["show", &format!("{snapshot}:cloud-only")]).unwrap(),
            "unreturned work"
        );
        assert!(
            run_cli(super::super::super::args::CloudCommand::CancelReturn {
                session: id.into(),
                epoch: 2
            })
            .is_err()
        );
        // Even a leftover staged activation cannot revive the old owner.
        let staged = inbox_dir().unwrap().join(format!("{id}.1"));
        std::fs::create_dir_all(staged).unwrap();
        assert!(
            run_activate(id, 1)
                .unwrap_err()
                .to_string()
                .contains("stale activation")
        );
        std::fs::remove_file(return_state_path(id, 2, "export.tar").unwrap()).unwrap();
        assert!(
            export_return_archive(id, 2, true)
                .unwrap_err()
                .to_string()
                .contains("possibly replaced checkout")
        );
    }

    #[test]
    fn legacy_export_does_not_require_completion_and_pending_export_can_cancel() {
        if isolated("legacy_export_does_not_require_completion_and_pending_export_can_cancel") {
            return;
        }
        for (id, require_completion) in [("legacy-client", false), ("new-client", true)] {
            let mut sess = session::Session::create_with_id(id.into(), None, None);
            sess.working_dir = None;
            sess.migration_epoch = 1;
            sess.save_prepared().unwrap();
            storage::write_session_lease(&storage::SessionLease {
                session_id: id.into(),
                epoch: 1,
                away_host: None,
                repo_root: None,
                last_sync_snapshot: None,
                last_sync_head: None,
                updated_at: String::new(),
            })
            .unwrap();
            let bytes = export_return_archive(id, 2, require_completion).unwrap();
            let lease = storage::read_session_lease(id).unwrap();
            assert_eq!(
                lease.away_host.as_deref(),
                Some(if require_completion {
                    "return-pending"
                } else {
                    "returned"
                })
            );
            let archive = tempfile::tempdir().unwrap();
            let tar = archive.path().join("return.tar");
            std::fs::write(&tar, bytes).unwrap();
            untar(&tar, archive.path()).unwrap();
            assert_eq!(
                archive.path().join("return-completion-v1").exists(),
                require_completion
            );
            if require_completion {
                run_cli(super::super::super::args::CloudCommand::CancelReturn {
                    session: id.into(),
                    epoch: 2,
                })
                .unwrap();
                assert!(storage::read_session_lease(id).unwrap().is_here());
                assert!(!return_state_path(id, 2, "export.tar").unwrap().exists());
            } else {
                assert!(reject_lease(Path::new("/repo"), &lease).is_ok());
            }
        }
    }

    #[test]
    fn durable_return_state_is_private_and_rejects_path_traversal() {
        assert!(return_state_path("../escape", 1, "export.tar").is_err());
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("return/export.tar");
        write_return_state(&path, b"first").unwrap();
        write_return_state(&path, b"second").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"second");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                std::fs::metadata(path.parent().unwrap())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
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
        assert!(same_repository(Path::new("/repo/subdir"), Path::new("/repo")).unwrap());
        assert!(same_repository(Path::new("/repo"), Path::new("/repo/subdir")).unwrap());
        assert!(!same_repository(Path::new("/repo-other"), Path::new("/repo")).unwrap());
    }
}
