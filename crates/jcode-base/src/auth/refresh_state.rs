use anyhow::Result;
use std::collections::BTreeMap;
use std::path::PathBuf;

const REFRESH_STATUS_FILE: &str = "auth-refresh-state.json";
const MAX_ERROR_CHARS: usize = 240;

pub use jcode_auth_types::ProviderRefreshRecord;

pub fn status_path() -> Result<PathBuf> {
    Ok(crate::storage::jcode_dir()?.join(REFRESH_STATUS_FILE))
}

pub fn load_all() -> BTreeMap<String, ProviderRefreshRecord> {
    let Ok(path) = status_path() else {
        return BTreeMap::new();
    };
    crate::storage::read_json(&path).unwrap_or_default()
}

pub fn get(provider_id: &str) -> Option<ProviderRefreshRecord> {
    load_all().get(provider_id).cloned()
}

pub fn record_success(provider_id: &str) -> Result<()> {
    let now_ms = chrono::Utc::now().timestamp_millis();
    upsert(
        provider_id,
        ProviderRefreshRecord {
            last_attempt_ms: now_ms,
            last_success_ms: Some(now_ms),
            last_error: None,
        },
    )
}

pub fn record_failure(provider_id: &str, error: impl AsRef<str>) -> Result<()> {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let mut message = error.as_ref().trim().to_string();
    if message.chars().count() > MAX_ERROR_CHARS {
        message = message.chars().take(MAX_ERROR_CHARS).collect::<String>();
        message.push('…');
    }
    let mut record = get(provider_id).unwrap_or(ProviderRefreshRecord {
        last_attempt_ms: now_ms,
        last_success_ms: None,
        last_error: None,
    });
    record.last_attempt_ms = now_ms;
    record.last_error = Some(message);
    upsert(provider_id, record)
}

pub fn format_record_label(record: &ProviderRefreshRecord) -> String {
    let age = age_label(record.last_attempt_ms);
    if let Some(error) = record.last_error.as_deref() {
        format!("failed {} ({})", age, error)
    } else if record.last_success_ms.is_some() {
        format!("ok {}", age)
    } else {
        format!("attempted {}", age)
    }
}

fn upsert(provider_id: &str, record: ProviderRefreshRecord) -> Result<()> {
    let mut records = load_all();
    records.insert(provider_id.to_string(), record);
    crate::storage::write_json(&status_path()?, &records)
}

fn age_label(checked_at_ms: i64) -> String {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let delta_ms = now_ms.saturating_sub(checked_at_ms).max(0);
    let delta_secs = delta_ms / 1000;
    match delta_secs {
        0..=89 => "just now".to_string(),
        90..=3599 => format!("{}m ago", delta_secs / 60),
        3600..=86_399 => format!("{}h ago", delta_secs / 3600),
        _ => format!("{}d ago", delta_secs / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_record_label_prefers_failure_details() {
        let record = ProviderRefreshRecord {
            last_attempt_ms: chrono::Utc::now().timestamp_millis(),
            last_success_ms: Some(chrono::Utc::now().timestamp_millis()),
            last_error: Some("refresh denied".to_string()),
        };
        assert!(format_record_label(&record).contains("failed"));
        assert!(format_record_label(&record).contains("refresh denied"));
    }

    #[test]
    fn format_record_label_reports_success() {
        let now_ms = chrono::Utc::now().timestamp_millis();
        let record = ProviderRefreshRecord {
            last_attempt_ms: now_ms,
            last_success_ms: Some(now_ms),
            last_error: None,
        };
        assert!(format_record_label(&record).starts_with("ok "));
    }
}
