use super::*;

fn read_backup(path: &Path) -> Result<Option<PersistedSidePanelState>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes).with_context(|| {
            format!("failed to parse side panel backup {}", path.display())
        })?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error)
            .with_context(|| format!("failed to read side panel backup {}", path.display())),
    }
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
    let previous = if path.exists() {
        Some(crate::storage::read_json::<PersistedSidePanelState>(&path)?)
    } else {
        None
    };
    let old_backup = read_backup(&backup_path)?;
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
    let mut retired = match previous {
        Some(state) => state,
        None => PersistedSidePanelState::default(),
    };
    if let Some(old_backup) = old_backup {
        merge_records(&mut retired, old_backup);
    }
    let mut protected = state;
    if let Some(backup) = backup {
        merge_records(&mut protected, backup);
    }
    retention::remove_replaced(session_id, &retired, &protected)
}
