use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReloadRecoveryDirective {
    pub reconnect_notice: Option<String>,
    pub continuation_message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SelfDevBuildCommand {
    pub program: String,
    pub args: Vec<String>,
    pub display: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SelfDevBuildTarget {
    Auto,
    Tui,
    Desktop,
    All,
}

impl SelfDevBuildTarget {
    pub fn parse(value: Option<&str>) -> Result<Self> {
        match value.unwrap_or("auto").trim().to_ascii_lowercase().as_str() {
            "" | "auto" => Ok(Self::Auto),
            "tui" | "jcode" => Ok(Self::Tui),
            "desktop" | "jcode-desktop" => Ok(Self::Desktop),
            "all" | "both" => Ok(Self::All),
            other => anyhow::bail!(
                "invalid selfdev build target `{}`; expected auto, tui, desktop, or all",
                other
            ),
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct BinaryVersionReport {
    pub version: Option<String>,
    pub git_hash: Option<String>,
}

/// Which binary to use.
#[derive(Debug, Clone)]
pub enum BinaryChoice {
    /// Use the stable version.
    Stable(String),
    /// Use the canary version for testing.
    Canary(String),
    /// Use current running binary because no versioned builds exist yet.
    Current,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceState {
    pub repo_scope: String,
    pub worktree_scope: String,
    pub short_hash: String,
    pub full_hash: String,
    pub dirty: bool,
    pub fingerprint: String,
    pub version_label: String,
    pub changed_paths: usize,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SelfDevCustomizationStatus {
    #[default]
    Active,
    Disabled,
    Superseded,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SelfDevCustomizationOutcomeStatus {
    AppliedCleanly,
    NeedsReview,
    Disabled,
    RepairedAutomatically,
    ValidationPassed,
    ValidationFailed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SelfDevCustomizationBuildMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_command: Option<SelfDevBuildCommand>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_fingerprint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SelfDevCustomizationProvenance {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_dir: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceState>,
    #[serde(default)]
    pub touched_paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff_stat: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SelfDevCustomizationValidation {
    #[serde(default)]
    pub commands: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_status: Option<SelfDevCustomizationOutcomeStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_output: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_validated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SelfDevCustomizationOutcome {
    pub status: SelfDevCustomizationOutcomeStatus,
    pub timestamp: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default)]
    pub validation_commands: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SelfDevCustomizationRecord {
    pub id: String,
    #[serde(default)]
    pub status: SelfDevCustomizationStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled_at: Option<DateTime<Utc>>,
    pub goal: String,
    pub expected_behavior: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rationale: Option<String>,
    #[serde(default)]
    pub update_hints: Vec<String>,
    #[serde(default)]
    pub provenance: SelfDevCustomizationProvenance,
    #[serde(default)]
    pub validation: SelfDevCustomizationValidation,
    #[serde(default)]
    pub build: SelfDevCustomizationBuildMetadata,
    #[serde(default)]
    pub outcomes: Vec<SelfDevCustomizationOutcome>,
}

impl SelfDevCustomizationRecord {
    pub fn new(
        id: impl Into<String>,
        goal: impl Into<String>,
        expected_behavior: impl Into<String>,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: id.into(),
            status: SelfDevCustomizationStatus::Active,
            created_at: now,
            updated_at: now,
            disabled_at: None,
            goal: goal.into(),
            expected_behavior: expected_behavior.into(),
            intent: None,
            rationale: None,
            update_hints: Vec::new(),
            provenance: SelfDevCustomizationProvenance::default(),
            validation: SelfDevCustomizationValidation::default(),
            build: SelfDevCustomizationBuildMetadata::default(),
            outcomes: Vec::new(),
        }
    }

    pub fn is_active(&self) -> bool {
        self.status == SelfDevCustomizationStatus::Active
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn customization_record_minimal_json_defaults() {
        let json = r#"{
            "id": "custom-1",
            "created_at": "2026-05-10T00:00:00Z",
            "updated_at": "2026-05-10T00:00:00Z",
            "goal": "Keep self-dev behavior",
            "expected_behavior": "Reload keeps the customization visible"
        }"#;

        let record: SelfDevCustomizationRecord = serde_json::from_str(json).unwrap();

        assert_eq!(record.status, SelfDevCustomizationStatus::Active);
        assert!(record.is_active());
        assert!(record.provenance.touched_paths.is_empty());
        assert!(record.validation.commands.is_empty());
        assert!(record.outcomes.is_empty());
    }

    #[test]
    fn customization_record_full_json_round_trips() {
        let mut record = SelfDevCustomizationRecord::new(
            "custom-2",
            "Remember local customization",
            "Status reports active records",
        );
        record.intent = Some("self-dev memory".to_string());
        record.rationale = Some("Agents need persistent context".to_string());
        record.update_hints.push("Review after update".to_string());
        record
            .provenance
            .touched_paths
            .push("src/tool/selfdev/mod.rs".to_string());
        record
            .validation
            .commands
            .push("cargo check -p jcode".to_string());
        record.outcomes.push(SelfDevCustomizationOutcome {
            status: SelfDevCustomizationOutcomeStatus::NeedsReview,
            timestamp: Utc::now(),
            detail: Some("active during update".to_string()),
            validation_commands: record.validation.commands.clone(),
        });

        let json = serde_json::to_string(&record).unwrap();
        let loaded: SelfDevCustomizationRecord = serde_json::from_str(&json).unwrap();

        assert_eq!(loaded, record);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PublishedBuild {
    pub version: String,
    pub source_fingerprint: String,
    pub versioned_path: PathBuf,
    pub current_link: PathBuf,
    pub launcher_link: PathBuf,
    pub previous_current_version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PendingActivation {
    pub session_id: String,
    pub new_version: String,
    pub previous_current_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_shared_server_version: Option<String>,
    pub source_fingerprint: Option<String>,
    pub requested_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DevBinarySourceMetadata {
    pub version_label: String,
    pub source_fingerprint: String,
    pub short_hash: String,
    pub full_hash: String,
    pub dirty: bool,
    pub changed_paths: usize,
}

impl From<&SourceState> for DevBinarySourceMetadata {
    fn from(source: &SourceState) -> Self {
        Self {
            version_label: source.version_label.clone(),
            source_fingerprint: source.fingerprint.clone(),
            short_hash: source.short_hash.clone(),
            full_hash: source.full_hash.clone(),
            dirty: source.dirty,
            changed_paths: source.changed_paths,
        }
    }
}

/// Status of a canary build being tested
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CanaryStatus {
    /// Build is currently being tested
    #[serde(alias = "Testing")]
    Testing,
    /// Build passed all tests and is ready for promotion
    #[serde(alias = "Passed")]
    Passed,
    /// Build failed testing
    #[serde(alias = "Failed")]
    Failed,
}

/// Information about a specific build version
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildInfo {
    /// Git commit hash (short)
    pub hash: String,
    /// Git commit hash (full)
    pub full_hash: String,
    /// Build timestamp
    pub built_at: DateTime<Utc>,
    /// Git commit message (first line)
    pub commit_message: Option<String>,
    /// Whether build is from dirty working tree
    pub dirty: bool,
    /// Stable fingerprint of the source state used to produce the build.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_fingerprint: Option<String>,
    /// Immutable published version label, if available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_label: Option<String>,
}

/// Information about a crash during canary testing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrashInfo {
    /// Build hash that crashed
    pub build_hash: String,
    /// Exit code
    pub exit_code: i32,
    /// Stderr output (truncated)
    pub stderr: String,
    /// Timestamp of crash
    pub crashed_at: DateTime<Utc>,
    /// Git diff that was being tested
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff: Option<String>,
}

/// Context saved before migrating to a canary build
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationContext {
    pub session_id: String,
    pub from_version: String,
    pub to_version: String,
    pub change_summary: Option<String>,
    pub diff: Option<String>,
    pub timestamp: DateTime<Utc>,
}
