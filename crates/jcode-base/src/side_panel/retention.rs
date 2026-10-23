use super::*;

/// Recent history defaults: at most 50 documents and 128 MiB of owned bodies per session.
pub const MAX_ARCHIVED_PAGES: usize = 50;
pub const MAX_ARCHIVED_BYTES: u64 = 128 * 1024 * 1024;

pub(super) fn atomic_content(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("content parent")?;
    crate::storage::ensure_dir(parent)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    use std::io::Write;
    temp.write_all(bytes)?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

pub(super) fn cache_loaded(
    session: &str,
    state: &mut PersistedSidePanelState,
    id: &str,
    content: &str,
    pdf: Option<&str>,
) -> Result<()> {
    let page = state
        .pages
        .iter_mut()
        .find(|p| p.id == id)
        .context("page missing")?;
    let revision = linked_file_revision(Path::new(&page.file_path));
    // Immutable generations ensure an interrupted metadata commit still references the old body.
    let nonce: u64 = rand::random();
    let path = session_dir(session)?.join(format!("{id}.{nonce:016x}.cache"));
    let bytes = match pdf {
        Some(data) => base64::engine::general_purpose::STANDARD.decode(data)?,
        None => content.as_bytes().to_vec(),
    };
    atomic_content(&path, &bytes)?;
    page.cache_path = Some(path.display().to_string());
    page.status.source_revision = revision;
    page.status.last_successful_update_ms = now_ms();
    page.status.stale_reason = None;
    page.updated_at_ms = page.status.last_successful_update_ms;
    Ok(())
}

pub(super) fn hydrate(state: PersistedSidePanelState) -> Result<SidePanelSnapshot> {
    hydrate_with_refresh(state, None)
}

fn hydrate_with_refresh(
    state: PersistedSidePanelState,
    force_page: Option<&str>,
) -> Result<SidePanelSnapshot> {
    let session = state
        .session_id
        .as_deref()
        .context("missing session identity")?;
    let mut state = load_state(session)?;
    let session = session.to_string();
    let mut changed = false;
    for i in 0..state.pages.len() {
        if state.pages[i].source != SidePanelPageSource::LinkedFile {
            continue;
        }
        let page = &state.pages[i];
        let revision = linked_file_revision(Path::new(&page.file_path));
        if revision == page.status.source_revision
            && page.cache_path.is_some()
            && page.status.stale_reason.is_none()
            && force_page != Some(page.id.as_str())
        {
            continue;
        }
        let result = read_page_content(Path::new(&page.file_path), page.format);
        match result {
            Ok((content, pdf)) => {
                let id = state.pages[i].id.clone();
                cache_loaded(&session, &mut state, &id, &content, pdf.as_deref())?;
                changed = true;
            }
            Err(error) => {
                let reason = format!("{error:#}");
                if state.pages[i].status.stale_reason.as_ref() != Some(&reason) {
                    state.pages[i].status.stale_reason = Some(reason);
                    changed = true;
                }
            }
        }
    }
    if changed {
        save_state(&session, &state)?;
    }
    state = load_state(&session)?;
    let mut budget = MAX_SESSION_PDF_BYTES;
    let pages = state
        .pages
        .into_iter()
        .filter(|p| p.source != SidePanelPageSource::Ephemeral)
        .map(|page| {
            let path = page.cache_path.as_deref().unwrap_or(&page.file_path);
            let (mut content, mut pdf_data) = hydrated_content(Path::new(path), page.format);
            enforce_pdf_budget(&mut content, &mut pdf_data, &mut budget);
            SidePanelPage {
                id: page.id,
                title: page.title,
                file_path: page.file_path,
                format: page.format,
                source: page.source,
                updated_at_ms: page.updated_at_ms,
                status: page.status,
                content,
                pdf_data,
            }
        })
        .collect();
    Ok(SidePanelSnapshot {
        session_id: Some(session),
        revision: state.revision,
        unavailable: None,
        archived: state.archived,
        focus_revision: state.focus_revision,
        focused_page_id: state.focused_page_id,
        pages,
    })
}

pub(super) fn refresh_snapshot(snapshot: &mut SidePanelSnapshot, force_page: Option<&str>) -> bool {
    let Some(session) = snapshot.session_id.clone() else {
        let mut changed = false;
        for page in &mut snapshot.pages {
            if page.source != SidePanelPageSource::LinkedFile {
                continue;
            }
            let revision = linked_file_revision(Path::new(&page.file_path));
            if page.status.source_revision == revision
                && page.status.stale_reason.is_none()
                && force_page != Some(page.id.as_str())
            {
                continue;
            }
            match read_page_content(Path::new(&page.file_path), page.format) {
                Ok((content, pdf)) => {
                    page.content = content;
                    page.pdf_data = pdf;
                    page.status.source_revision = revision;
                    page.updated_at_ms = now_ms().max(page.updated_at_ms.saturating_add(1));
                    page.status.last_successful_update_ms = page.updated_at_ms;
                    page.status.stale_reason = None;
                    changed = true;
                }
                Err(error) => {
                    let reason = Some(format!("{error:#}"));
                    changed |= page.status.stale_reason != reason;
                    page.status.stale_reason = reason;
                }
            }
        }
        let mut budget = MAX_SESSION_PDF_BYTES;
        for page in &mut snapshot.pages {
            enforce_pdf_budget(&mut page.content, &mut page.pdf_data, &mut budget);
        }
        return changed;
    };
    let result = store_lock().and_then(|_lock| {
        load_state(&session).and_then(|state| hydrate_with_refresh(state, force_page))
    });
    match result {
        Ok(next) => {
            let changed = *snapshot != next;
            *snapshot = next;
            changed
        }
        Err(error) => {
            let reason = Some(format!("{error:#}"));
            let changed = snapshot.unavailable != reason;
            snapshot.unavailable = reason;
            changed
        }
    }
}

pub fn history(session: &str) -> Result<Vec<SidePanelHistoryEntry>> {
    let _lock = store_lock()?;
    Ok(load_state(session)?.archived)
}

fn record(entry: SidePanelHistoryEntry) -> PersistedSidePanelPage {
    PersistedSidePanelPage {
        id: entry.id,
        title: entry.title,
        file_path: entry.file_path,
        format: entry.format,
        source: entry.source,
        updated_at_ms: entry.updated_at_ms,
        status: entry.status,
        cache_path: entry.cache_path,
    }
}
fn entry(page: PersistedSidePanelPage) -> SidePanelHistoryEntry {
    SidePanelHistoryEntry {
        id: page.id,
        title: page.title,
        file_path: page.file_path,
        format: page.format,
        source: page.source,
        updated_at_ms: page.updated_at_ms,
        status: page.status,
        cache_path: page.cache_path,
        archived_at_ms: now_ms(),
    }
}
fn refocus(state: &mut PersistedSidePanelState) {
    if !state
        .pages
        .iter()
        .any(|p| Some(&p.id) == state.focused_page_id.as_ref())
    {
        state.focused_page_id = state.pages.first().map(|p| p.id.clone());
    }
}
pub(super) fn remove_replaced(
    session: &str,
    old: &PersistedSidePanelState,
    next: &PersistedSidePanelState,
) -> Result<()> {
    let dir = session_dir(session)?;
    let pages = |state: &PersistedSidePanelState| {
        state
            .pages
            .clone()
            .into_iter()
            .chain(state.archived.clone().into_iter().map(record))
            .collect::<Vec<_>>()
    };
    let old_pages = pages(old);
    let next_pages = pages(next);
    let protected: Vec<&str> = next_pages
        .iter()
        .flat_map(|p| std::iter::once(p.file_path.as_str()).chain(p.cache_path.as_deref()))
        .chain(
            old_pages
                .iter()
                .filter(|p| p.source == SidePanelPageSource::LinkedFile)
                .map(|p| p.file_path.as_str()),
        )
        .collect();
    for page in &old_pages {
        let candidate = page.cache_path.as_deref().or_else(|| {
            (page.source == SidePanelPageSource::Managed).then_some(page.file_path.as_str())
        });
        if let Some(candidate) = candidate {
            let path = Path::new(candidate);
            if path.parent() == Some(dir.as_path()) && !protected.contains(&candidate) {
                match std::fs::remove_file(path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => {
                        return Err(error).with_context(|| {
                            format!("failed to retire side panel body {}", path.display())
                        });
                    }
                }
            }
        }
    }
    Ok(())
}

pub fn archive_page(session: &str, id: &str) -> Result<SidePanelSnapshot> {
    let _lock = store_lock()?;
    validate_page_id(id)?;
    let mut state = load_state(session)?;
    let index = state
        .pages
        .iter()
        .position(|p| p.id == id)
        .context("Side panel page not found")?;
    let page = state.pages.remove(index);
    if page.source != SidePanelPageSource::Ephemeral {
        state.archived.retain(|p| p.id != id);
        state.archived.insert(0, entry(page));
    }
    while state.archived.len() > MAX_ARCHIVED_PAGES
        || state
            .archived
            .iter()
            .map(|p| {
                std::fs::metadata(p.cache_path.as_deref().unwrap_or(&p.file_path))
                    .map(|m| m.len())
                    .unwrap_or(0)
            })
            .sum::<u64>()
            > MAX_ARCHIVED_BYTES
    {
        state.archived.pop();
    }
    refocus(&mut state);
    save_state(session, &state)?;
    hydrate(state)
}

pub fn reopen_page(session: &str, id: &str) -> Result<SidePanelSnapshot> {
    let _lock = store_lock()?;
    validate_page_id(id)?;
    let mut state = load_state(session)?;
    if let Some(index) = state.archived.iter().position(|p| p.id == id) {
        let page = record(state.archived.remove(index));
        state.pages.retain(|p| p.id != id);
        state.pages.push(page);
    }
    let page = state
        .pages
        .iter_mut()
        .find(|p| p.id == id)
        .context("Side panel page not found")?;
    page.status.last_opened_at_ms = now_ms();
    state.focused_page_id = Some(id.to_string());
    state.focus_revision = next_focus_revision(state.focus_revision);
    save_state(session, &state)?;
    hydrate(state)
}

pub fn purge_page(session: &str, id: &str) -> Result<SidePanelSnapshot> {
    let _lock = store_lock()?;
    validate_page_id(id)?;
    let mut state = load_state(session)?;
    let count = state.pages.len() + state.archived.len();
    state.pages.retain(|p| p.id != id);
    state.archived.retain(|p| p.id != id);
    anyhow::ensure!(
        count != state.pages.len() + state.archived.len(),
        "Side panel page not found: {id}"
    );
    refocus(&mut state);
    save_state(session, &state)?;
    hydrate(state)
}

pub fn inherit_for_recovery(from: &str, to: &str) -> Result<SidePanelSnapshot> {
    let _lock = store_lock()?;
    anyhow::ensure!(from != to, "recovery requires a new session");
    let source = load_state(from)?;
    let mut target = load_state(to)?;
    anyhow::ensure!(
        target.pages.is_empty() && target.archived.is_empty(),
        "recovery target is not empty"
    );
    for (mut page, archived_at) in
        source
            .pages
            .into_iter()
            .map(|page| (page, None))
            .chain(source.archived.into_iter().map(|entry| {
                let at = entry.archived_at_ms;
                (record(entry), Some(at))
            }))
    {
        if page.source == SidePanelPageSource::Ephemeral {
            continue;
        }
        validate_page_id(&page.id)?;
        let old = page.cache_path.as_deref().unwrap_or(&page.file_path);
        let path = session_dir(to)?.join(format!("{}.recovered.cache", page.id));
        atomic_content(&path, &std::fs::read(old)?)?;
        page.cache_path = Some(path.display().to_string());
        page.status.restored_from_session = Some(from.to_string());
        page.status.stale_reason = Some("Restored after recovery".to_string());
        if let Some(at) = archived_at {
            let mut archived = entry(page);
            archived.archived_at_ms = at;
            target.archived.push(archived);
        } else {
            target.pages.push(page);
        }
    }
    target.focused_page_id = source.focused_page_id;
    save_state(to, &target)?;
    hydrate(target)
}
