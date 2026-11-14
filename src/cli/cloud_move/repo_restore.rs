use super::*;

pub(super) fn restore_repo_contents(
    repo: &RepoManifest,
    root: &Path,
    bundle_str: &str,
    snap_ref: &str,
    ignored_tar: &Path,
    session_id: &str,
) -> Result<String> {
    let root = root.to_path_buf();
    let bundle_str = bundle_str.to_owned();
    let snap_ref = snap_ref.to_owned();
    // An incremental bundle may omit the branch advertisement when that
    // branch's tip is already a prerequisite on this host. The snapshot has
    // the complete parent chain we need, so fetch it rather than requiring an
    // advertised branch. Verify prerequisites before touching checkout refs.
    git(&root, &["bundle", "verify", &bundle_str])?;
    let mut fetch_args = vec!["fetch", "--quiet", "--force", bundle_str.as_str()];
    let snap_spec = format!("{snap_ref}:{snap_ref}");
    fetch_args.push(&snap_spec);
    git(&root, &fetch_args)?;
    git(
        &root,
        &["cat-file", "-e", &format!("{}^{{commit}}", repo.head)],
    )?;

    match &repo.branch {
        Some(branch) => {
            git(
                &root,
                &["symbolic-ref", "HEAD", &format!("refs/heads/{branch}")],
            )?;
        }
        None => {
            git(&root, &["update-ref", "--no-deref", "HEAD", &repo.head])?;
        }
    }
    // Branch at original HEAD, working tree + index = snapshot, so uncommitted
    // work stays uncommitted exactly as it was locally.
    git(&root, &["reset", "--quiet", "--hard", &repo.head])?;
    git(&root, &["read-tree", "--reset", "-u", &snap_ref])?;
    git(&root, &["reset", "--quiet", "--mixed", &repo.head])?;
    git(
        &root,
        &[
            "update-ref",
            &format!("{SNAPSHOT_REF_PREFIX}/{session_id}/base"),
            &repo.snapshot,
        ],
    )?;
    if let Some(name) = &repo.git_user_name
        && git(&root, &["config", "user.name"]).is_err()
    {
        git(&root, &["config", "user.name", name])?;
    }
    if let Some(email) = &repo.git_user_email
        && git(&root, &["config", "user.email"]).is_err()
    {
        git(&root, &["config", "user.email", email])?;
    }
    if ignored_tar.is_file() {
        let mut command = Command::new("tar");
        command.arg("-xf").arg(ignored_tar).arg("-C").arg(&root);
        run_ok(command, "restore ignored files")?;
    }
    git(&root, &["rev-parse", &snap_ref])
}
