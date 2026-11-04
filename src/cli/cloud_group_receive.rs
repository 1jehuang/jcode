/// Called only after the ordinary receive entry point extracts the archive.
pub(super) fn receive(staging: tempfile::TempDir) -> Result<()> {
    let group: Group = read_json(&staging.path().join("group.json"))?;
    validate(&group, staging.path())?;
    if group.preview {
        verify_preview_repo(&group, staging.path())?;
        println!("{}", serde_json::to_string(&receipt(&group)?)?);
        return Ok(());
    }
    let ids = group
        .members
        .iter()
        .map(|m| m.manifest.session_id.clone())
        .collect::<Vec<_>>();
    let _locks = lock_sessions(&ids)?;
    let manifest = &root(&group)?.manifest;
    let directory = inbox_dir()?.join(format!("{}.{}", group.coordinator, manifest.epoch));
    let installed = group_dir(&group.coordinator)?.join("received.json");
    // A lost upload/activation response must never cause the repository or an
    // already-running member's transcript to be overwritten on retry.
    for existing in [installed, directory.join("group.json")] {
        if existing.exists() {
            let previous: Group = read_json(&existing)?;
            if serde_json::to_vec(&previous)? != serde_json::to_vec(&group)? {
                bail!("coordinated migration identity already exists with different contents");
            }
            println!("{}", serde_json::to_string(&receipt(&group)?)?);
            return Ok(());
        }
    }
    for member in &group.members {
        let id = &member.manifest.session_id;
        if let Some(lease) = storage::read_session_lease(id) {
            if lease.epoch >= member.manifest.epoch || lease.is_here() {
                bail!(
                    "destination session {id} has conflicting ownership at epoch {}",
                    lease.epoch
                );
            }
        } else if session::session_path(id)?.exists() {
            bail!("destination already has an unmanaged session {id}; refusing to overwrite it");
        }
    }
    daemon_command("check_import", &group.swarm)?;
    let repo = manifest.repo.as_ref().context("group repository missing")?;
    let restored = restore_repo(
        repo,
        &staging.path().join("repo.bundle"),
        &staging.path().join("ignored.tar"),
        &group.coordinator,
    )?;
    if restored != repo.snapshot {
        bail!("restored group repository snapshot mismatch");
    }
    let kept = staging.keep();
    std::fs::rename(&kept, &directory)?;
    println!("{}", serde_json::to_string(&receipt(&group)?)?);
    Ok(())
}

pub(super) fn activate(coordinator: &str, epoch: u64) -> Result<()> {
    checked_id(coordinator)?;
    let directory = inbox_dir()?.join(format!("{coordinator}.{epoch}"));
    let group: Group = read_json(&directory.join("group.json"))
        .context("group staging is missing; source ownership remains fenced, restore the staged archive before retrying")?;
    if group.preview {
        bail!("a preview cannot be activated");
    }
    if group.coordinator != coordinator || root(&group)?.manifest.epoch != epoch {
        bail!("group activation coordinator/epoch mismatch");
    }
    let ids = group
        .members
        .iter()
        .map(|m| m.manifest.session_id.clone())
        .collect::<Vec<_>>();
    let _locks = lock_sessions(&ids)?;
    validate(&group, &directory)?;
    let record = group_dir(coordinator)?.join("received.json");
    if record.exists() {
        let installed: Group = read_json(&record)?;
        if serde_json::to_vec(&installed)? != serde_json::to_vec(&group)? {
            bail!("destination group record does not match staged group");
        }
    }
    // Validate every account before touching any member. Credentials remain
    // outside the archive and are installed by the private SSH channel.
    for member in &group.members {
        let sess: session::Session = read_json(
            &directory
                .join("members")
                .join(&member.manifest.session_id)
                .join("session.json"),
        )?;
        if let Some(account) = &member.manifest.auth_account {
            let credentials =
                session_credentials(&sess)?.context("group member lost its OAuth route")?;
            if credentials.provider() != account.provider || credentials.label() != account.label {
                bail!("group member OAuth identity mismatch");
            }
        }
        if let Some(lease) = storage::read_session_lease(&sess.id) {
            if lease.epoch > member.manifest.epoch {
                bail!("destination member has a newer migration epoch");
            }
            if lease.epoch == member.manifest.epoch && lease.is_here() && !record.exists() {
                bail!("destination member is active without this group transaction");
            }
        }
    }
    if !record.exists() {
        daemon_command("reserve", &group.swarm)?;
        // Install all sessions under blocking leases. Only after transcripts,
        // todos and swarm topology are durable may any destination member run.
        for member in &group.members {
            write_lease(&member_lease(
                member,
                Some("group-activation-pending".into()),
            ))?;
        }
        for member in &group.members {
            let stage = directory.join("members").join(&member.manifest.session_id);
            let sess: session::Session = read_json(&stage.join("session.json"))?;
            if member.todos_sha256.is_none() {
                let path = todo_file(&sess.id)?;
                if path.exists() {
                    std::fs::remove_file(path)?;
                }
            }
            install_session(&sess, &stage)?;
            storage::write_json_secret(
                &transferred_accounts_path(&sess.id)?,
                &member.manifest.auth_accounts,
            )?;
        }
        if let Ok(limits) = std::fs::read(directory.join("limits.json")) {
            crate::usage::import_limits_snapshot(&limits);
        }
        import_group(&group.swarm)?;
        mark_members(&group)?;
        durable_json(&record, &group)?;
    }
    // This loop is intentionally restartable and never writes transcripts.
    // A member activated before a transport/process failure may have new turns.
    for member in &group.members {
        let lease = storage::read_session_lease(&member.manifest.session_id)
            .context("installed group member has no destination lease")?;
        if lease.epoch != member.manifest.epoch {
            bail!("destination group member epoch changed; refusing stale activation");
        }
        if !lease.is_here() {
            write_lease(&member_lease(member, None))?;
        }
    }
    eprintln!(
        "activated {} group members at coordinator {coordinator}",
        group.members.len()
    );
    Ok(())
}
