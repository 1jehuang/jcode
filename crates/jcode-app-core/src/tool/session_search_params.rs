//! Typed parsing and validation for `session_search` tool inputs.
//!
//! Each helper turns one raw `SearchInput` field into a bounded option or an
//! actionable error message; `Tool::execute` calls them before running a search.

use super::RoleFilter;
use chrono::{DateTime, NaiveDate, Utc};

pub(super) fn validate_bounded_usize(
    value: Option<i64>,
    default: usize,
    min: usize,
    max: usize,
    name: &str,
) -> std::result::Result<usize, String> {
    let Some(value) = value else {
        return Ok(default);
    };
    if value < min as i64 || value > max as i64 {
        return Err(format!(
            "{name} must be between {min} and {max}; received {value}."
        ));
    }
    Ok(value as usize)
}

pub(super) fn parse_role_filter(
    raw: Option<&str>,
) -> std::result::Result<Option<RoleFilter>, String> {
    let Some(raw) = raw.map(str::trim).filter(|raw| !raw.is_empty()) else {
        return Ok(None);
    };
    if raw.eq_ignore_ascii_case("all") {
        return Ok(None);
    }
    RoleFilter::parse(raw).map(Some).ok_or_else(|| {
        format!("role must be one of all, user, assistant, or metadata; received {raw}.")
    })
}

/// Decide which project scope a search covers.
///
/// The agent-facing default is this session's own working directory. Without
/// it an agent in project A could read transcripts from every project on the
/// machine, which is the cross-project leak isolation invariant 4 rules out.
/// An explicit "*" is the documented opt-out for genuinely global recall.
///
/// When the session has no working directory the filter stays `None`. It is
/// never filled in from the daemon's cwd: invariant 1 says a session-scoped
/// path may not fall back to whichever project started the process, and
/// inventing a scope here would be worse than an unscoped, honest search.
pub(super) fn resolve_working_dir_filter(
    requested: Option<&str>,
    session_working_dir: Option<&std::path::Path>,
) -> Option<String> {
    let requested = requested.map(str::trim).filter(|raw| !raw.is_empty());

    if let Some(raw) = requested {
        // "*" means every project. Any other value is the agent's explicit choice
        // and is passed through for the matcher to interpret.
        if raw == "*" {
            return None;
        }
        return Some(raw.to_string());
    }

    session_working_dir.map(|dir| dir.to_string_lossy().into_owned())
}

pub(super) fn normalize_optional_filter(raw: Option<String>) -> Option<String> {
    raw.map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
}

pub(super) fn normalize_source_filter(
    raw: Option<&str>,
) -> std::result::Result<Option<String>, String> {
    let Some(source) = raw.map(str::trim).filter(|source| !source.is_empty()) else {
        return Ok(None);
    };
    let normalized = source.to_ascii_lowercase();
    match normalized.as_str() {
        "all" => Ok(None),
        "jcode" | "claude" | "claude-code" | "codex" | "pi" | "opencode" | "cursor" => {
            Ok(Some(normalized.replace("claude-code", "claude")))
        }
        _ => Err(format!(
            "source must be one of all, jcode, claude, codex, pi, opencode, or cursor; received {source}."
        )),
    }
}

pub(super) fn parse_datetime_filter(
    raw: Option<&str>,
    name: &str,
) -> std::result::Result<Option<DateTime<Utc>>, String> {
    let Some(raw) = raw.map(str::trim).filter(|raw| !raw.is_empty()) else {
        return Ok(None);
    };
    if let Ok(dt) = DateTime::parse_from_rfc3339(raw) {
        return Ok(Some(dt.with_timezone(&Utc)));
    }
    if let Ok(date) = NaiveDate::parse_from_str(raw, "%Y-%m-%d") {
        let Some(naive) = date.and_hms_opt(0, 0, 0) else {
            return Err(format!("{name} has an invalid date: {raw}."));
        };
        return Ok(Some(DateTime::from_naive_utc_and_offset(naive, Utc)));
    }
    Err(format!(
        "{name} must be an RFC3339 timestamp or YYYY-MM-DD date; received {raw}."
    ))
}
