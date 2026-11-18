//! Input filter parsing and validation for session search.

use super::*;

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
