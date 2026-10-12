//! Shared persistence and retention finalization for external session imports.

use super::normalize_imported_history;
use crate::message::{ContentBlock, Role};
use crate::session::{Session, SessionStatus, StoredMessage};
use anyhow::Result;
use chrono::{DateTime, Utc};

pub(super) fn finalize_imported_session(
    session: Session,
    created_at: DateTime<Utc>,
    updated_at: Option<DateTime<Utc>>,
) -> Result<Session> {
    finalize_imported_session_with_omissions(session, created_at, updated_at, 0)
}

pub(super) fn finalize_imported_session_with_omissions(
    mut session: Session,
    created_at: DateTime<Utc>,
    updated_at: Option<DateTime<Utc>>,
    omitted: usize,
) -> Result<Session> {
    // Never overwrite a jcode-side continuation with a shorter external
    // transcript. This protection applies to every importer, not only Claude.
    if crate::session::session_exists(&session.id)
        && let Ok(mut existing) = Session::load(&session.id)
        && existing.messages.len() > session.messages.len()
    {
        if normalize_imported_history(&mut existing, false) {
            existing.save()?;
        }
        return Ok(existing);
    }

    let original_messages = session.messages.len().saturating_add(omitted);
    session.created_at = created_at;
    session.updated_at = updated_at.unwrap_or(created_at);
    session.last_active_at = updated_at.or(Some(created_at));
    session.status = SessionStatus::Closed;
    normalize_imported_history(&mut session, true);
    if omitted > 0 {
        let mut messages = std::mem::take(&mut session.messages);
        messages.insert(0, StoredMessage {
            id: crate::id::new_id("message"),
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: format!("[Imported session: {omitted} older messages were omitted for a fast, provider-safe resume. Use session_search to inspect earlier external history.]"),
                cache_control: None,
            }],
            display_role: None, timestamp: None, tool_duration_ms: None, token_usage: None,
        });
        session.replace_messages(messages);
    }
    session.save()?;
    crate::logging::info(&format!(
        "Imported session prepared: source_messages={original_messages} kept_messages={}",
        session.messages.len()
    ));
    Ok(session)
}
