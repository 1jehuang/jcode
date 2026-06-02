use anyhow::Result;
use std::collections::BTreeMap;
use std::path::PathBuf;

const VALIDATION_STATUS_FILE: &str = "auth-validation.json";

pub use jcode_auth_types::ProviderValidationRecord;

pub fn status_path() -> Result<PathBuf> {
    Ok(crate::storage::jcode_dir()?.join(VALIDATION_STATUS_FILE))
}

pub fn load_all() -> BTreeMap<String, ProviderValidationRecord> {
    let Ok(path) = status_path() else {
        return BTreeMap::new();
    };
    crate::storage::read_json(&path).unwrap_or_default()
}

pub fn get(provider_id: &str) -> Option<ProviderValidationRecord> {
    load_all().get(provider_id).cloned()
}

pub fn save(provider_id: &str, record: ProviderValidationRecord) -> Result<()> {
    let mut records = load_all();
    records.insert(provider_id.to_string(), record);
    crate::storage::write_json(&status_path()?, &records)
}

pub fn status_label(provider_id: &str) -> Option<String> {
    get(provider_id).map(|record| format_record_label(&record))
}

pub fn format_record_label(record: &ProviderValidationRecord) -> String {
    let age = age_label(record.checked_at_ms);
    let base = if record.success {
        if record.tool_smoke_ok == Some(true) {
            "runtime + tool validated"
        } else if record.provider_smoke_ok == Some(true) {
            "runtime validated"
        } else {
            "validated"
        }
    } else {
        "validation failed"
    };
    format!("{} ({})", base, age)
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
    fn format_record_label_prefers_tool_validated_wording() {
        let record = ProviderValidationRecord {
            checked_at_ms: chrono::Utc::now().timestamp_millis(),
            success: true,
            provider_smoke_ok: Some(true),
            tool_smoke_ok: Some(true),
            summary: "ok".to_string(),
        };
        assert!(format_record_label(&record).starts_with("runtime + tool validated"));
    }

    #[test]
    fn format_record_label_reports_failures() {
        let record = ProviderValidationRecord {
            checked_at_ms: chrono::Utc::now().timestamp_millis(),
            success: false,
            provider_smoke_ok: Some(false),
            tool_smoke_ok: Some(false),
            summary: "provider smoke failed".to_string(),
        };
        assert!(format_record_label(&record).starts_with("validation failed"));
    }
}
