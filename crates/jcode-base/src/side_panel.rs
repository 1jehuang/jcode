use anyhow::{Context, Result};
use base64::Engine as _;
use std::io::Read as _;

/// Maximum unencoded PDF payload size.
pub const MAX_PDF_BYTES: u64 = 20 * 1024 * 1024;
/// Aggregate decoded PDF budget keeps base64 snapshots within the transport cap.
pub const MAX_SESSION_PDF_BYTES: u64 = 32 * 1024 * 1024;
pub use jcode_side_panel_types::{
    PersistedSidePanelPage, PersistedSidePanelState, SidePanelPage, SidePanelPageFormat,
    SidePanelPageSource, SidePanelSnapshot, snapshot_is_empty,
};
use std::hash::{Hash as _, Hasher as _};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn snapshot_for_session(session_id: &str) -> Result<SidePanelSnapshot> {
    let state = load_state(session_id)?;
    hydrate_snapshot(state)
}

pub fn write_markdown_page(
    session_id: &str,
    page_id: &str,
    title: Option<&str>,
    content: &str,
    focus: bool,
) -> Result<SidePanelSnapshot> {
    write_page(session_id, page_id, title, content, focus, false)
}

pub fn append_markdown_page(
    session_id: &str,
    page_id: &str,
    title: Option<&str>,
    content: &str,
    focus: bool,
) -> Result<SidePanelSnapshot> {
    write_page(session_id, page_id, title, content, focus, true)
}

pub fn load_markdown_file(
    session_id: &str,
    page_id: &str,
    title: Option<&str>,
    source_path: &Path,
    focus: bool,
) -> Result<SidePanelSnapshot> {
    validate_markdown_source_path(source_path)?;
    load_file(session_id, page_id, title, source_path, focus)
}

/// Load a linked Markdown or PDF file without copying or modifying the source.
pub fn load_file(
    session_id: &str,
    page_id: &str,
    title: Option<&str>,
    source_path: &Path,
    focus: bool,
) -> Result<SidePanelSnapshot> {
    validate_page_id(page_id)?;
    let format = source_format(source_path)?;

    let (_, pdf_data) = read_page_content(source_path, format)?;
    let source_path =
        std::fs::canonicalize(source_path).unwrap_or_else(|_| source_path.to_path_buf());

    let mut state = load_state(session_id)?;
    let now = now_ms();
    validate_pdf_budget(
        &state,
        page_id,
        pdf_data.as_deref().map(pdf_byte_len).unwrap_or(0),
    )?;

    upsert_page_record(
        &mut state,
        page_id,
        title,
        PageRecordLocation {
            file_path: &source_path,
            source: SidePanelPageSource::LinkedFile,
            format,
        },
        now,
        focus,
    );
    save_state(session_id, &state)?;

    hydrate_snapshot(state)
}

pub fn refresh_linked_page_content(
    snapshot: &mut SidePanelSnapshot,
    page_id: Option<&str>,
) -> bool {
    let target_page_id = page_id.or(snapshot.focused_page_id.as_deref());
    let mut changed = false;

    for page in &mut snapshot.pages {
        if page.source != SidePanelPageSource::LinkedFile {
            continue;
        }
        if let Some(target_page_id) = target_page_id
            && page.id != target_page_id
            // PDF pages share a session-wide budget. A nonfocused PDF growing
            // or shrinking can change whether the focused page can be loaded.
            && page.format != SidePanelPageFormat::Pdf
        {
            continue;
        }

        let next_revision = linked_file_revision(Path::new(&page.file_path));
        if next_revision == page.updated_at_ms {
            continue;
        }

        let (content, pdf_data) = hydrated_content(Path::new(&page.file_path), page.format);
        page.content = content;
        page.pdf_data = pdf_data;
        page.updated_at_ms = next_revision;
        changed = true;
    }

    if changed {
        let mut budget = MAX_SESSION_PDF_BYTES;
        for page in &mut snapshot.pages {
            // Reconsider all PDFs when budget changes, including a previously
            // downgraded page whose own file revision has not changed.
            if page.source == SidePanelPageSource::LinkedFile
                && page.format == SidePanelPageFormat::Pdf
            {
                (page.content, page.pdf_data) =
                    hydrated_content(Path::new(&page.file_path), page.format);
                page.updated_at_ms = linked_file_revision(Path::new(&page.file_path));
            }
            enforce_pdf_budget(&mut page.content, &mut page.pdf_data, &mut budget);
        }
    }
    changed
}

pub fn focus_page(session_id: &str, page_id: &str) -> Result<SidePanelSnapshot> {
    validate_page_id(page_id)?;
    let mut state = load_state(session_id)?;
    if state.pages.iter().any(|page| page.id == page_id) {
        state.focused_page_id = Some(page_id.to_string());
        state.focus_revision = next_focus_revision(state.focus_revision);
        save_state(session_id, &state)?;
        hydrate_snapshot(state)
    } else {
        anyhow::bail!("Side panel page not found: {}", page_id);
    }
}

pub fn delete_page(session_id: &str, page_id: &str) -> Result<SidePanelSnapshot> {
    validate_page_id(page_id)?;
    let mut state = load_state(session_id)?;
    let before = state.pages.len();
    state.pages.retain(|page| page.id != page_id);
    if state.pages.len() == before {
        anyhow::bail!("Side panel page not found: {}", page_id);
    }

    let page_path = session_dir(session_id)?.join(format!("{}.md", page_id));
    let _ = std::fs::remove_file(page_path);

    if state.focused_page_id.as_deref() == Some(page_id) {
        state.focused_page_id = state
            .pages
            .iter()
            .max_by_key(|page| page.updated_at_ms)
            .map(|page| page.id.clone());
    }

    save_state(session_id, &state)?;
    hydrate_snapshot(state)
}

pub fn status_output(snapshot: &SidePanelSnapshot) -> String {
    if snapshot.pages.is_empty() {
        return "Side panel: empty".to_string();
    }

    let focused = snapshot
        .focused_page()
        .map(|page| page.id.as_str())
        .unwrap_or("none");
    let mut out = format!(
        "Side panel: {} page{}\nFocused: {}\n",
        snapshot.pages.len(),
        if snapshot.pages.len() == 1 { "" } else { "s" },
        focused
    );

    for page in &snapshot.pages {
        let focus_marker = if snapshot.focused_page_id.as_deref() == Some(page.id.as_str()) {
            "*"
        } else {
            " "
        };
        out.push_str(&format!(
            "{} {} ({})\n  title: {}\n  source: {}\n  file: {}\n",
            focus_marker,
            page.id,
            page.format.as_str(),
            page.title,
            page.source.as_str(),
            page.file_path
        ));
    }

    out.trim_end().to_string()
}

fn write_page(
    session_id: &str,
    page_id: &str,
    title: Option<&str>,
    content: &str,
    focus: bool,
    append: bool,
) -> Result<SidePanelSnapshot> {
    validate_page_id(page_id)?;
    let dir = session_dir(session_id)?;
    crate::storage::ensure_dir(&dir)?;

    let page_path = dir.join(format!("{}.md", page_id));
    let mut state = load_state(session_id)?;
    let now = now_ms();
    if append
        && state
            .pages
            .iter()
            .any(|page| page.id == page_id && page.format == SidePanelPageFormat::Pdf)
    {
        anyhow::bail!("cannot append Markdown to a PDF panel; use write to replace it");
    }

    let combined_content = if append && page_path.exists() {
        let mut existing = std::fs::read_to_string(&page_path)
            .with_context(|| format!("failed to read {}", page_path.display()))?;
        if !existing.is_empty() && !existing.ends_with('\n') {
            existing.push('\n');
        }
        existing.push_str(content);
        existing
    } else {
        content.to_string()
    };

    std::fs::write(&page_path, &combined_content)
        .with_context(|| format!("failed to write {}", page_path.display()))?;

    upsert_page_record(
        &mut state,
        page_id,
        title,
        PageRecordLocation {
            file_path: &page_path,
            source: SidePanelPageSource::Managed,
            format: SidePanelPageFormat::Markdown,
        },
        now,
        focus,
    );

    save_state(session_id, &state)?;
    hydrate_snapshot(state)
}

/// Where a side panel page's content lives and how it is rendered.
struct PageRecordLocation<'a> {
    file_path: &'a Path,
    source: SidePanelPageSource,
    format: SidePanelPageFormat,
}

fn upsert_page_record(
    state: &mut PersistedSidePanelState,
    page_id: &str,
    title: Option<&str>,
    location: PageRecordLocation<'_>,
    updated_at_ms: u64,
    focus: bool,
) {
    let PageRecordLocation {
        file_path,
        source,
        format,
    } = location;
    let file_path = file_path.display().to_string();
    if let Some(existing) = state.pages.iter_mut().find(|page| page.id == page_id) {
        existing.title = title
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .unwrap_or(existing.title.as_str())
            .to_string();
        existing.file_path = file_path;
        existing.format = format;
        existing.source = source;
        existing.updated_at_ms = updated_at_ms;
    } else {
        state.pages.push(PersistedSidePanelPage {
            id: page_id.to_string(),
            title: title
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .unwrap_or(page_id)
                .to_string(),
            file_path,
            format,
            source,
            updated_at_ms,
        });
    }

    state.pages.sort_by(|a, b| {
        b.updated_at_ms
            .cmp(&a.updated_at_ms)
            .then_with(|| a.id.cmp(&b.id))
    });

    if focus {
        state.focus_revision = next_focus_revision(state.focus_revision);
    }
    if focus {
        state.focused_page_id = Some(page_id.to_string());
    }
}

fn hydrate_snapshot(state: PersistedSidePanelState) -> Result<SidePanelSnapshot> {
    let mut pdf_budget = MAX_SESSION_PDF_BYTES;
    let pages = state
        .pages
        .into_iter()
        .map(|page| {
            let (mut content, mut pdf_data) = if page.source == SidePanelPageSource::Ephemeral {
                (String::new(), None)
            } else {
                hydrated_content(Path::new(&page.file_path), page.format)
            };
            enforce_pdf_budget(&mut content, &mut pdf_data, &mut pdf_budget);
            let updated_at_ms = match page.source {
                SidePanelPageSource::Managed => page.updated_at_ms,
                SidePanelPageSource::LinkedFile => linked_file_revision(Path::new(&page.file_path)),
                SidePanelPageSource::Ephemeral => page.updated_at_ms,
            };
            SidePanelPage {
                id: page.id,
                title: page.title,
                file_path: page.file_path,
                format: page.format,
                source: page.source,
                content: if page.source == SidePanelPageSource::Ephemeral {
                    String::new()
                } else {
                    content
                },
                pdf_data,
                updated_at_ms,
            }
        })
        .collect();

    Ok(SidePanelSnapshot {
        focus_revision: state.focus_revision,
        focused_page_id: state.focused_page_id,
        pages,
    })
}

fn load_state(session_id: &str) -> Result<PersistedSidePanelState> {
    let path = state_file(session_id)?;
    if !path.exists() {
        return Ok(PersistedSidePanelState::default());
    }
    crate::storage::read_json(&path)
}

fn save_state(session_id: &str, state: &PersistedSidePanelState) -> Result<()> {
    let path = state_file(session_id)?;
    crate::storage::write_json_fast(&path, state)
}

fn session_dir(session_id: &str) -> Result<PathBuf> {
    let base = crate::storage::jcode_dir()?.join("side_panel");
    Ok(base.join(session_id))
}

fn state_file(session_id: &str) -> Result<PathBuf> {
    Ok(session_dir(session_id)?.join("index.json"))
}

fn validate_page_id(page_id: &str) -> Result<()> {
    let page_id = page_id.trim();
    if page_id.is_empty() {
        anyhow::bail!("page_id cannot be empty");
    }
    if page_id.len() > 80 {
        anyhow::bail!("page_id is too long (max 80 characters)");
    }
    if !page_id
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
    {
        anyhow::bail!("page_id must use only ASCII letters, digits, underscore, dash, or dot");
    }
    if page_id.contains("..") {
        anyhow::bail!("page_id cannot contain '..'");
    }
    if Path::new(page_id).components().count() != 1 {
        anyhow::bail!("page_id cannot contain path separators");
    }
    Ok(())
}

fn validate_markdown_source_path(path: &Path) -> Result<()> {
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase());

    let is_markdown = matches!(
        ext.as_deref(),
        Some("md") | Some("markdown") | Some("mdown") | Some("mkd") | Some("mkdn")
    );

    if !is_markdown {
        anyhow::bail!(
            "side_panel load only supports markdown files (.md, .markdown, .mdown, .mkd, .mkdn): {}",
            path.display()
        );
    }

    Ok(())
}

fn next_focus_revision(previous: u64) -> u64 {
    now_ms().max(previous.saturating_add(1))
}

fn pdf_byte_len(encoded: &str) -> u64 {
    ((encoded.len() / 4) * 3 - encoded.bytes().rev().take_while(|b| *b == b'=').count()) as u64
}

fn validate_pdf_budget(
    state: &PersistedSidePanelState,
    replacing: &str,
    new_bytes: u64,
) -> Result<()> {
    let existing: u64 = state
        .pages
        .iter()
        .filter(|p| p.id != replacing && p.format == SidePanelPageFormat::Pdf)
        .filter_map(|p| std::fs::metadata(&p.file_path).ok())
        .map(|m| m.len())
        .filter(|len| *len <= MAX_PDF_BYTES)
        .sum();
    anyhow::ensure!(
        existing.saturating_add(new_bytes) <= MAX_SESSION_PDF_BYTES,
        "session PDF payload exceeds aggregate 32 MiB limit"
    );
    Ok(())
}

fn enforce_pdf_budget(content: &mut String, data: &mut Option<String>, budget: &mut u64) {
    if let Some(encoded) = data {
        let size = pdf_byte_len(encoded);
        if size > *budget {
            *data = None;
            *content =
                "Unable to load PDF: session PDF payload exceeds aggregate 32 MiB limit.".into();
        } else {
            *budget -= size;
        }
    }
}

fn source_format(path: &Path) -> Result<SidePanelPageFormat> {
    if path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"))
    {
        return Ok(SidePanelPageFormat::Pdf);
    }
    validate_markdown_source_path(path)
        .with_context(|| "load supports Markdown and PDF (.pdf) files")?;
    Ok(SidePanelPageFormat::Markdown)
}

fn read_page_content(path: &Path, format: SidePanelPageFormat) -> Result<(String, Option<String>)> {
    match format {
        SidePanelPageFormat::Markdown => Ok((
            std::fs::read_to_string(path)
                .with_context(|| format!("failed to read {}", path.display()))?,
            None,
        )),
        SidePanelPageFormat::Pdf => {
            let file = std::fs::File::open(path)
                .with_context(|| format!("failed to read PDF {}", path.display()))?;
            anyhow::ensure!(
                file.metadata()?.len() <= MAX_PDF_BYTES,
                "PDF exceeds 20 MiB limit: {}",
                path.display()
            );
            // Bound the actual read too, in case the source grows after metadata.
            let mut bytes = Vec::new();
            file.take(MAX_PDF_BYTES + 1)
                .read_to_end(&mut bytes)
                .with_context(|| format!("failed to read PDF {}", path.display()))?;
            anyhow::ensure!(
                bytes.len() as u64 <= MAX_PDF_BYTES,
                "PDF exceeds 20 MiB limit: {}",
                path.display()
            );
            anyhow::ensure!(
                bytes.starts_with(b"%PDF-"),
                "invalid PDF signature: {}",
                path.display()
            );
            Ok((
                format!(
                    "PDF document: `{}`\n\nOpen this panel in the desktop app to view the PDF.",
                    path.display()
                ),
                Some(base64::engine::general_purpose::STANDARD.encode(bytes)),
            ))
        }
    }
}

fn hydrated_content(path: &Path, format: SidePanelPageFormat) -> (String, Option<String>) {
    read_page_content(path, format).unwrap_or_else(|err| {
        (
            format!("Unable to load {} document: {err:#}", format.as_str()),
            None,
        )
    })
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|dur| dur.as_millis() as u64)
        .unwrap_or(0)
}

fn linked_file_revision(path: &Path) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut hasher);

    match std::fs::metadata(path) {
        Ok(metadata) => {
            metadata.len().hash(&mut hasher);
            metadata.permissions().readonly().hash(&mut hasher);
            metadata
                .modified()
                .ok()
                .and_then(|ts| ts.duration_since(UNIX_EPOCH).ok())
                .map(|dur| (dur.as_secs(), dur.subsec_nanos()))
                .hash(&mut hasher);
            "present".hash(&mut hasher);
        }
        Err(_) => {
            "missing".hash(&mut hasher);
        }
    }

    hasher.finish()
}

#[cfg(test)]
#[path = "side_panel_tests.rs"]
mod side_panel_tests;
