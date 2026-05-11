use super::{
    SelfDevCustomizationOutcome, SelfDevCustomizationOutcomeStatus, SelfDevCustomizationRecord,
    SelfDevCustomizationStatus,
};
use anyhow::Result;
use chrono::Utc;
use jcode_storage as storage;
use std::path::PathBuf;

fn customizations_dir() -> Result<PathBuf> {
    let dir = storage::jcode_dir()?.join("selfdev").join("customizations");
    storage::ensure_dir(&dir)?;
    Ok(dir)
}

pub fn customization_records_dir() -> Result<PathBuf> {
    let dir = customizations_dir()?.join("records");
    storage::ensure_dir(&dir)?;
    Ok(dir)
}

pub fn customization_patches_dir() -> Result<PathBuf> {
    let dir = customizations_dir()?.join("patches");
    storage::ensure_dir(&dir)?;
    Ok(dir)
}

pub fn customization_record_path(id: &str) -> Result<PathBuf> {
    Ok(customization_records_dir()?.join(format!("{}.json", sanitize_record_id(id))))
}

pub fn customization_patch_path(id: &str) -> Result<PathBuf> {
    Ok(customization_patches_dir()?.join(format!("{}.patch", sanitize_record_id(id))))
}

pub fn sanitize_record_id(id: &str) -> String {
    let mut clean = String::with_capacity(id.len());
    for ch in id.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
            clean.push(ch);
        } else {
            clean.push('-');
        }
    }
    let clean = clean.trim_matches('-');
    if clean.is_empty() {
        format!("customization-{}", Utc::now().timestamp_millis())
    } else {
        clean.chars().take(96).collect()
    }
}

pub fn create_customization_record(
    mut record: SelfDevCustomizationRecord,
    patch: Option<&str>,
) -> Result<SelfDevCustomizationRecord> {
    record.id = sanitize_record_id(&record.id);
    let now = Utc::now();
    record.updated_at = now;
    if record.created_at > now {
        record.created_at = now;
    }

    if let Some(patch) = patch.filter(|patch| !patch.trim().is_empty()) {
        let patch_path = customization_patch_path(&record.id)?;
        storage::write_text_secret(&patch_path, patch)?;
        record.provenance.patch_path = Some(patch_path);
    }

    save_customization_record(&record)?;
    Ok(record)
}

pub fn save_customization_record(record: &SelfDevCustomizationRecord) -> Result<()> {
    storage::write_json(&customization_record_path(&record.id)?, record)
}

pub fn load_customization_record(id: &str) -> Result<Option<SelfDevCustomizationRecord>> {
    let path = customization_record_path(id)?;
    if path.exists() {
        Ok(Some(storage::read_json(&path)?))
    } else {
        Ok(None)
    }
}

pub fn list_customization_records() -> Result<Vec<SelfDevCustomizationRecord>> {
    let dir = customization_records_dir()?;
    let mut records = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        if let Ok(record) = storage::read_json::<SelfDevCustomizationRecord>(&path) {
            records.push(record);
        }
    }
    records.sort_by(|a, b| {
        b.updated_at
            .cmp(&a.updated_at)
            .then_with(|| a.id.cmp(&b.id))
    });
    Ok(records)
}

pub fn list_active_customization_records() -> Result<Vec<SelfDevCustomizationRecord>> {
    Ok(list_customization_records()?
        .into_iter()
        .filter(SelfDevCustomizationRecord::is_active)
        .collect())
}

pub fn disable_customization_record(
    id: &str,
    detail: Option<String>,
) -> Result<SelfDevCustomizationRecord> {
    let mut record = load_customization_record(id)?
        .ok_or_else(|| anyhow::anyhow!("customization record `{}` not found", id))?;
    let now = Utc::now();
    record.status = SelfDevCustomizationStatus::Disabled;
    record.disabled_at = Some(now);
    record.updated_at = now;
    record.outcomes.push(SelfDevCustomizationOutcome {
        status: SelfDevCustomizationOutcomeStatus::Disabled,
        timestamp: now,
        detail,
        validation_commands: Vec::new(),
    });
    save_customization_record(&record)?;
    Ok(record)
}

pub fn append_customization_outcome(
    id: &str,
    outcome: SelfDevCustomizationOutcome,
) -> Result<SelfDevCustomizationRecord> {
    let mut record = load_customization_record(id)?
        .ok_or_else(|| anyhow::anyhow!("customization record `{}` not found", id))?;
    record.updated_at = Utc::now();
    record.outcomes.push(outcome);
    save_customization_record(&record)?;
    Ok(record)
}

pub fn summarize_customization_update_state() -> Result<Vec<SelfDevCustomizationOutcome>> {
    Ok(list_active_customization_records()?
        .into_iter()
        .map(|record| SelfDevCustomizationOutcome {
            status: SelfDevCustomizationOutcomeStatus::NeedsReview,
            timestamp: Utc::now(),
            detail: Some(format!(
                "Customization `{}` is active and should be reviewed against this update.",
                record.id
            )),
            validation_commands: record.validation.commands,
        })
        .collect())
}
