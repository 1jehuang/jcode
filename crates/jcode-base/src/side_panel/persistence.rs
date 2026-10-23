use super::*;

fn read_index(path: &Path) -> Result<Option<PersistedSidePanelState>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes).with_context(|| {
            format!("failed to parse side panel index {}", path.display())
        })?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error)
            .with_context(|| format!("failed to read side panel index {}", path.display())),
    }
}

fn recover_backup(session_id: &str, path: &Path) -> Result<Option<PersistedSidePanelState>> {
    let backup_path = path.with_extension("bak");
    let Some(mut state) = read_index(&backup_path)? else {
        return Ok(None);
    };
    state.session_id = Some(session_id.to_string());
    retention::atomic_content(path, &serde_json::to_vec(&state)?)?;
    crate::logging::warn(&format!(
        "Recovered side panel index {} atomically from {}",
        path.display(),
        backup_path.display()
    ));
    Ok(Some(state))
}

pub(super) fn load_state(session_id: &str) -> Result<PersistedSidePanelState> {
    let path = state_file(session_id)?;
    let mut state = PersistedSidePanelState::default();
    match read_index(&path) {
        Ok(Some(primary)) => state = primary,
        Ok(None) => {
            if let Some(backup) = recover_backup(session_id, &path)? {
                state = backup;
            }
        }
        Err(error) if error.downcast_ref::<serde_json::Error>().is_some() => {
            state = recover_backup(session_id, &path)
                .with_context(|| format!("invalid primary side panel index: {error:#}"))?
                .with_context(|| {
                    format!("no valid backup for primary side panel index: {error:#}")
                })?;
        }
        Err(error) => return Err(error),
    }
    state.session_id = Some(session_id.to_string());
    Ok(state)
}

fn merge_records(target: &mut PersistedSidePanelState, other: PersistedSidePanelState) {
    target.pages.extend(other.pages);
    target.archived.extend(other.archived);
}

pub(super) fn save_state(session_id: &str, state: &PersistedSidePanelState) -> Result<()> {
    let path = state_file(session_id)?;
    let backup_path = path.with_extension("bak");
    let mut state = state.clone();
    state.session_id = Some(session_id.to_string());
    state
        .pages
        .retain(|p| p.source != SidePanelPageSource::Ephemeral);
    state
        .archived
        .retain(|p| p.source != SidePanelPageSource::Ephemeral);
    state.revision = state.revision.saturating_add(1);
    let previous = read_index(&path)?;
    let old_backup = match read_index(&backup_path) {
        Ok(backup) => backup,
        Err(error) if previous.is_some() && error.downcast_ref::<serde_json::Error>().is_some() => {
            crate::logging::warn(&format!(
                "Repairing corrupt redundant side panel backup {} from validated primary: {error:#}",
                backup_path.display()
            ));
            None
        }
        Err(error) => return Err(error),
    };
    crate::storage::write_json_fast(&path, &state)?;

    // Keep the previous version's bodies, but never let fallback undo a purge or eviction.
    // Publish the sanitized backup before acknowledging success or retiring any body.
    let mut backup = previous.clone();
    if let Some(backup) = &mut backup {
        let retained: Vec<&str> = state
            .pages
            .iter()
            .map(|p| p.id.as_str())
            .chain(state.archived.iter().map(|p| p.id.as_str()))
            .collect();
        backup.pages.retain(|p| retained.contains(&p.id.as_str()));
        backup
            .archived
            .retain(|p| retained.contains(&p.id.as_str()));
        if !backup
            .pages
            .iter()
            .any(|p| Some(&p.id) == backup.focused_page_id.as_ref())
        {
            backup.focused_page_id = None;
        }
        retention::atomic_content(&backup_path, &serde_json::to_vec(backup)?)?;
    }
    let mut retired = PersistedSidePanelState::default();
    for generation in previous.into_iter().chain(old_backup) {
        merge_records(&mut retired, generation);
    }
    let mut protected = state;
    if let Some(backup) = backup {
        merge_records(&mut protected, backup);
    }
    retention::remove_replaced(session_id, &retired, &protected)
}
