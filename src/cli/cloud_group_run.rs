/// Migrate the persisted swarm containing `session_ref`, plus explicitly opted
/// in independent sessions. Every selected member must share one Git checkout.
pub(crate) fn run_migrate(
    session_ref: Option<&str>,
    target: &Target,
    allow_active: bool,
    dry_run: bool,
    include_sessions: &[String],
) -> Result<MoveReport> {
    let requested = resolve_session_id(session_ref)?;
    if include_sessions.is_empty() && standalone_without_daemon(&requested)? {
        return run_move(Some(&requested), target, allow_active, dry_run);
    }
    let host = target.resolve_host()?;
    let (coordinator, ids, mut swarm) = select_group(&requested, include_sessions)?;
    swarm["destination_host"] = serde_json::Value::String(host.clone());
    if ids.len() == 1
        && include_sessions.is_empty()
        && swarm["swarms"].as_array().is_some_and(Vec::is_empty)
        && !group_dir(&coordinator)?.join("journal.json").exists()
    {
        return run_move(Some(&requested), target, allow_active, dry_run);
    }
    let _locks = lock_sessions(&ids)?;
    let directory = group_dir(&coordinator)?;
    let journal_path = directory.join("journal.json");
    if journal_path.exists() {
        let old: Journal = read_json(&journal_path)?;
        if root(&old.group)?.manifest.target_host != host {
            bail!(
                "unfinished or completed migration targets another host; refusing to change ownership"
            );
        }
        if old
            .group
            .members
            .iter()
            .map(|m| &m.manifest.session_id)
            .collect::<BTreeSet<_>>()
            != ids.iter().collect::<BTreeSet<_>>()
        {
            bail!(
                "migration membership differs from durable transaction; retry with the original member selection"
            );
        }
        if old.phase != Phase::Preparing {
            if dry_run {
                bail!("a committed migration already exists; retry without --dry-run to recover");
            }
            if old.phase == Phase::Complete {
                return report(&old.group, false);
            }
            verify_upload(&old.group, &directory.join("stage"), target, &host)?;
            return finish_activation(old, &directory, target, &host);
        }
        restore_preparing(&old)?;
        std::fs::remove_dir_all(&directory)?;
        drop(_locks);
        return run_migrate(session_ref, target, allow_active, dry_run, include_sessions);
    }
    for id in &ids {
        reject_member_return(id)?;
        if storage::read_session_lease(id).is_some_and(|l| !l.is_here()) {
            bail!("session {id} already lives elsewhere");
        }
        if storage::streaming_session_ids().contains(id) && !(allow_active && *id == requested) {
            bail!("group member {id} is mid-turn; wait for it to finish before migrating");
        }
    }
    target.preflight(&host)?;
    let capabilities = retry_ssh(&host, "group protocol probe", || {
        target.run_remote(&host, &["cloud", "group-protocol"], None)
    })
    .context("cloud host must support coordinated migration; update its jcode")?;
    let version: serde_json::Value = serde_json::from_slice(&capabilities)?;
    if version.get("version").and_then(|v| v.as_u64()) != Some(GROUP_VERSION as u64) {
        bail!("cloud host does not support coordinated migration protocol {GROUP_VERSION}");
    }
    let mut sessions = ids
        .iter()
        .map(|id| session::Session::load(id))
        .collect::<Result<Vec<_>>>()?;
    let mut repo_root: Option<PathBuf> = None;
    for sess in &sessions {
        let working_dir = sess
            .working_dir
            .as_deref()
            .context("group member has no working directory")?;
        let root = PathBuf::from(git(
            Path::new(working_dir),
            &["rev-parse", "--show-toplevel"],
        )?)
        .canonicalize()?;
        if repo_root.as_ref().is_some_and(|r| *r != root) {
            bail!("all swarm and opted-in sessions must share one repository checkout");
        }
        repo_root = Some(root);
    }
    let accounts = cloud_accounts()?;
    let selection: crate::server::GroupTransferBundle = serde_json::from_value(swarm.clone())?;
    let previous = selection
        .sessions
        .iter()
        .map(|s| s.previous_lease.clone())
        .collect::<Vec<_>>();
    let excluded_session_ids =
        excluded_local_sessions(repo_root.as_deref().context("missing repository")?, &ids)?;
    let mut group = Group {
        version: GROUP_VERSION,
        preview: dry_run,
        excluded_session_ids,
        coordinator,
        members: Vec::new(),
        swarm,
    };
    for (sess, selected) in sessions.iter_mut().zip(&selection.sessions) {
        let auth_account = session_credentials(sess)?.map(|credentials| CloudAccount {
            provider: credentials.provider(),
            label: credentials.label().to_string(),
        });
        if let Some(account) = &auth_account {
            sess.route_api_method = Some(account.route());
        }
        let epoch = selected.epoch;
        if selected.session_id != sess.id {
            bail!("selected session order mismatch");
        }
        group.members.push(Member {
            manifest: MoveManifest {
                format_version: FORMAT_VERSION,
                session_id: sess.id.clone(),
                epoch,
                source_host: local_host_label(),
                target_host: host.clone(),
                created_at: chrono::Utc::now().to_rfc3339(),
                repo: None,
                env: ENV_ALLOWLIST
                    .iter()
                    .filter_map(|key| match std::env::var(key) {
                        Ok(value) => Some(Ok((key.to_string(), value))),
                        Err(std::env::VarError::NotPresent) => None,
                        Err(std::env::VarError::NotUnicode(_)) => Some(Err(anyhow::anyhow!(
                            "cloud environment variable has invalid encoding"
                        ))),
                    })
                    .collect::<Result<_>>()?,
                toolchain_hints: toolchain_hints(repo_root.as_deref()),
                not_transferred: describe_local_only_state(&sess.id),
                mcp_servers: configured_mcp_servers(),
                auth_account,
                auth_accounts: accounts.clone(),
            },
            session_sha256: String::new(),
            todos_sha256: None,
        });
    }
    // Dry runs have no persistent transaction and never fence source leases.
    let temporary = tempfile::tempdir()?;
    let stage = if dry_run {
        temporary.path().to_path_buf()
    } else {
        directory.join("stage")
    };
    std::fs::create_dir_all(&stage)?;
    let mut journal = Journal {
        group,
        previous,
        phase: Phase::Preparing,
    };
    if !dry_run {
        save_journal(&directory, &journal)?;
    }
    let prepared = (|| -> Result<()> {
        if !dry_run {
            mark_members(&journal.group)?;
            prepare_group(&mut journal.group)?;
            save_journal(&directory, &journal)?;
            for member in &journal.group.members {
                write_lease(&member_lease(member, Some(host.clone())))?;
            }
        }
        // No member may start a new turn now. Reload from disk after fencing
        // rather than shipping the earlier selection/preflight copies.
        sessions = ids
            .iter()
            .map(|id| session::Session::load(id))
            .collect::<Result<Vec<_>>>()?;
        let working_dir = repo_root.as_ref().context("group requires a repository")?;
        let repo = prepare_repo_snapshot(working_dir, &journal.group.coordinator, &stage)?
            .context("group requires a repository")?;
        create_repo_bundle(
            &repo,
            &journal.group.coordinator,
            &stage,
            common_remote_commit(target, &host, &repo).as_deref(),
        )?;
        for (sess, member) in sessions.iter_mut().zip(&mut journal.group.members) {
            let mut member_repo = repo.clone();
            member_repo.working_dir = sess
                .working_dir
                .clone()
                .context("missing member working directory")?;
            member.manifest.repo = Some(member_repo);
            if let Some(account) = &member.manifest.auth_account {
                sess.route_api_method = Some(account.route());
            }
            sess.migration_epoch = member.manifest.epoch;
            let dangling = dangling_tool_use_ids(sess);
            let notice = group_arrival_notice(&member.manifest, &journal.group.coordinator, &ids);
            append_arrival(sess, &dangling, &host, &notice);
            let member_dir = stage.join("members").join(&sess.id);
            std::fs::create_dir_all(&member_dir)?;
            storage::write_json(&member_dir.join("session.json"), sess)?;
            member.session_sha256 = digest(&member_dir.join("session.json"))?;
            copy_if_exists(&todo_file(&sess.id)?, &member_dir.join("todos.json"))?;
            member.todos_sha256 = member_dir
                .join("todos.json")
                .exists()
                .then(|| digest(&member_dir.join("todos.json")))
                .transpose()?;
            let memory = crate::memory::project_memory_file(Path::new(
                &member
                    .manifest
                    .repo
                    .as_ref()
                    .context("missing member repository")?
                    .working_dir,
            ))?;
            copy_if_exists(&memory, &member_dir.join("project_memory.json"))?;
        }
        std::fs::write(
            stage.join("limits.json"),
            crate::usage::export_limits_snapshot(),
        )?;
        durable_json(&stage.join("group.json"), &journal.group)?;
        validate(&journal.group, &stage)?;
        tar_dir(
            &stage,
            &stage.join("move.tar"),
            &[
                "group.json",
                "members",
                "limits.json",
                "repo.bundle",
                "ignored.tar",
            ],
        )?;
        if !dry_run {
            save_journal(&directory, &journal)?;
        }
        transfer_and_verify(&journal.group, &stage, target, &host, dry_run)?;
        Ok(())
    })();
    if let Err(error) = prepared {
        if !dry_run {
            restore_preparing(&journal).context("pre-activation failure: restoring source ownership failed; retry migration to recover")?;
            std::fs::remove_dir_all(&directory)?;
        }
        return Err(error);
    }
    if dry_run {
        return report(&journal.group, true);
    }
    journal.phase = Phase::Activating;
    save_journal(&directory, &journal)?;
    finish_activation(journal, &directory, target, &host)
}

fn write_lease(lease: &storage::SessionLease) -> Result<()> {
    checked_id(&lease.session_id)?;
    durable_json(
        &storage::jcode_dir()?
            .join("session_leases")
            .join(format!("{}.json", lease.session_id)),
        lease,
    )
}

fn transfer_and_verify(
    group: &Group,
    stage: &Path,
    target: &Target,
    host: &str,
    dry_run: bool,
) -> Result<()> {
    if !dry_run {
        let mut baselines = Vec::new();
        for account in &root(group)?.manifest.auth_accounts {
            let payload = transfer::export_cloud_account_at(
                &storage::jcode_dir()?,
                account.provider,
                Some(&account.label),
            )?;
            retry_ssh(host, "group credentials", || {
                target.exchange_credentials(host, account, Some(payload.as_bytes()))
            })?;
            baselines.push(CloudAuthBaseline {
                account: account.clone(),
                fingerprint: credential_fingerprint(&payload),
            });
        }
        let router = export_router_credentials()?;
        if !router.is_empty() {
            retry_ssh(host, "group provider credentials", || {
                target.send_router_credentials(host, router.as_bytes())
            })?;
        }
        for member in &group.members {
            storage::write_json_secret(&baseline_path(&member.manifest.session_id)?, &baselines)?;
        }
    }
    verify_upload(group, stage, target, host)
}

fn verify_upload(group: &Group, stage: &Path, target: &Target, host: &str) -> Result<()> {
    let out = retry_ssh(host, "group upload", || {
        target.run_remote(
            host,
            &["cloud", "receive", "--json"],
            Some(&stage.join("move.tar")),
        )
    })?;
    let received: GroupReceipt =
        serde_json::from_slice(&out).context("invalid group receive report")?;
    if received != receipt(group)? {
        bail!("cloud group copy failed identity, transcript, todo or snapshot verification");
    }
    Ok(())
}

fn finish_activation(
    mut journal: Journal,
    directory: &Path,
    target: &Target,
    host: &str,
) -> Result<MoveReport> {
    let group = &journal.group;
    // Even partially written source fences are completed before retrying a
    // possibly successful remote activation. Never resurrect a source member.
    mark_members(group)?;
    for member in &group.members {
        write_lease(&member_lease(member, Some(host.to_owned())))?;
    }
    let epoch = root(group)?.manifest.epoch.to_string();
    retry_ssh(host, "group activation", || target.run_remote(host,
        &["cloud", "activate", "--session", &group.coordinator, "--epoch", &epoch], None))
        .context("group activation unconfirmed; all source members remain fenced. Retry the same cloud migrate command to recover safely")?;
    journal.phase = Phase::Complete;
    save_journal(directory, &journal)?;
    if let Some(repo) = &root(&journal.group)?.manifest.repo {
        if let Err(error) = environment::sync_saved(Path::new(&repo.root), target) {
            step(
                "environment",
                &format!("group migrated; saved environment synchronization needs retry: {error}"),
            );
        }
    }
    step(
        "done",
        &format!(
            "{} sessions now live on `{host}`",
            journal.group.members.len()
        ),
    );
    report(&journal.group, false)
}

fn report(group: &Group, dry_run: bool) -> Result<MoveReport> {
    let manifest = &root(group)?.manifest;
    Ok(MoveReport {
        session_id: group.coordinator.clone(),
        members: group
            .members
            .iter()
            .map(|m| m.manifest.session_id.clone())
            .collect(),
        excluded_session_ids: group.excluded_session_ids.clone(),
        host: manifest.target_host.clone(),
        epoch: manifest.epoch,
        repo_root: manifest.repo.as_ref().map(|r| r.root.clone()),
        snapshot: manifest.repo.as_ref().map(|r| r.snapshot.clone()),
        dry_run,
        attach_command: format!("jcode cloud attach --session {}", group.coordinator),
        return_command: "Group return is not implemented; do not return individual members".into(),
    })
}
