fn daemon_command(action: &str, payload: &serde_json::Value) -> Result<serde_json::Value> {
    let command = format!(
        "group_transfer:{action}:{}",
        serde_json::to_string(payload)?
    );
    let action = action.to_owned();
    std::thread::spawn(move || -> Result<serde_json::Value> {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
        runtime.block_on(async move {
            tokio::time::timeout(std::time::Duration::from_secs(30), async move {
                if !crate::server_spawn::is_running().await {
                    crate::server_spawn::spawn_default_server().await
                        .context("could not start daemon for coordinated migration")?;
                }
                let mut client = crate::server::Client::connect_debug().await
                    .context("coordinated migration requires a running local daemon with the group-transfer API")?;
                let request = client.debug_command(&command, None).await?;
                loop {
                    match client.read_event().await? {
                        crate::protocol::ServerEvent::DebugResponse { id, ok, output } if id == request => {
                            if !ok { bail!("group transfer {action}: {output}"); }
                            return serde_json::from_str(&output).context("invalid group-transfer response");
                        }
                        crate::protocol::ServerEvent::Error { id, message, .. } if id == request => bail!(message),
                        _ => {},
                    }
                }
            }).await.context("group-transfer daemon response timed out")?
        })
    }).join().map_err(|_| anyhow::anyhow!("group-transfer RPC thread panicked"))?
}

fn select_group(
    requested: &str,
    includes: &[String],
) -> Result<(String, Vec<String>, serde_json::Value)> {
    // Recovery cannot call inspect first: correctly fenced sessions are no
    // longer locally runnable and the daemon must reject a fresh prepare.
    let marker = member_marker(requested)?;
    let coordinator = if marker.exists() {
        let marker: serde_json::Value = read_json(&marker)?;
        marker["coordinator"]
            .as_str()
            .context("invalid group membership marker")?
            .to_owned()
    } else {
        requested.to_owned()
    };
    let journal_path = group_dir(&coordinator)?.join("journal.json");
    if journal_path.exists() {
        let journal: Journal = read_json(&journal_path)?;
        let ids: Vec<String> = journal
            .group
            .members
            .iter()
            .map(|m| m.manifest.session_id.clone())
            .collect();
        for include in includes {
            let id = resolve_session_id(Some(include))?;
            if !ids.contains(&id) {
                bail!("cannot add session {id} while recovering an existing group migration");
            }
        }
        return Ok((coordinator, ids, journal.group.swarm));
    }
    let mut selected = vec![requested.to_owned()];
    for include in includes {
        let id = resolve_session_id(Some(include))?;
        if !selected.contains(&id) {
            selected.push(id);
        }
    }
    let bundle = daemon_command("inspect", &serde_json::json!({"session_ids": selected}))?;
    let parsed: crate::server::GroupTransferBundle = serde_json::from_value(bundle.clone())?;
    let ids: Vec<_> = parsed
        .sessions
        .iter()
        .map(|s| s.session_id.clone())
        .collect();
    let coordinator = parsed
        .swarms
        .iter()
        .find(|s| {
            s.coordinator_session_id.as_deref() == Some(requested)
                || s.members.iter().any(|m| m.record.session_id == requested)
        })
        .and_then(|s| s.coordinator_session_id.clone())
        .unwrap_or_else(|| requested.to_owned());
    Ok((coordinator, ids, bundle))
}

fn prepare_group(group: &mut Group) -> Result<()> {
    let ids: Vec<_> = group
        .members
        .iter()
        .map(|m| m.manifest.session_id.clone())
        .collect();
    let host = root(group)?.manifest.target_host.clone();
    group.swarm["destination_host"] = serde_json::Value::String(host.clone());
    let prepared = daemon_command(
        "prepare",
        &serde_json::json!({
            "session_ids": ids, "destination_host": host, "expected": group.swarm,
        }),
    )?;
    let bundle: crate::server::GroupTransferBundle = serde_json::from_value(prepared.clone())?;
    if bundle.sessions.len() != group.members.len()
        || group.members.iter().any(|member| {
            !bundle.sessions.iter().any(|s| {
                s.session_id == member.manifest.session_id && s.epoch == member.manifest.epoch
            })
        })
    {
        bail!("daemon prepared an unexpected group; source fences retained for recovery");
    }
    group.swarm = prepared;
    Ok(())
}

fn import_group(bundle: &serde_json::Value) -> Result<()> {
    daemon_command("import", bundle).map(|_| ())
}

fn standalone_without_daemon(id: &str) -> Result<bool> {
    if member_marker(id)?.exists() || group_dir(id)?.join("journal.json").exists() {
        return Ok(false);
    }
    let running = std::thread::spawn(|| -> Result<bool> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        Ok(runtime.block_on(crate::server_spawn::is_running()))
    })
    .join()
    .map_err(|_| anyhow::anyhow!("daemon probe thread panicked"))??;
    if running {
        return Ok(false);
    }
    for directory in [
        storage::durable_state_dir().join("swarm"),
        storage::runtime_dir().join("jcode-swarm-state"),
    ] {
        if !directory.exists() {
            continue;
        }
        for entry in std::fs::read_dir(directory)? {
            let path = entry?.path();
            if !path.is_file() {
                continue;
            }
            let Ok(value) = read_json::<serde_json::Value>(&path) else {
                // Unknown/corrupt persisted topology must not silently turn a
                // group into a single-session handoff.
                return Ok(false);
            };
            if json_mentions_session(&value, id) {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

fn json_mentions_session(value: &serde_json::Value, id: &str) -> bool {
    match value {
        serde_json::Value::String(value) => value == id,
        serde_json::Value::Array(values) => values.iter().any(|v| json_mentions_session(v, id)),
        serde_json::Value::Object(values) => values.values().any(|v| json_mentions_session(v, id)),
        _ => false,
    }
}
