use super::*;
use crate::memory::{MemoryCategory, MemoryEntry, MemoryManager, TrustLevel};
use std::collections::BTreeSet;

const DIFF_STAT_LIMIT: usize = 4000;

impl SelfDevTool {
    pub(super) async fn do_record_customization(
        &self,
        params: SelfDevInput,
        ctx: &ToolContext,
    ) -> Result<ToolOutput> {
        let goal = non_empty(params.goal, "goal")?;
        let expected_behavior = non_empty(params.expected_behavior, "expected_behavior")?;
        let repo_dir = Self::resolve_repo_dir(ctx.working_dir.as_deref())
            .ok_or_else(|| anyhow::anyhow!("Could not find jcode repository"))?;
        let source = Self::requested_source_state(&repo_dir)?;
        let id = params
            .id
            .unwrap_or_else(|| format!("customization-{}", uuid::Uuid::new_v4().simple()));

        let mut touched_paths = detected_touched_paths(&repo_dir)?;
        if let Some(paths) = params.touched_paths {
            touched_paths.extend(paths.into_iter().filter(|path| !path.trim().is_empty()));
        }

        let diff = if Self::is_test_session() {
            String::new()
        } else {
            build::current_git_patch_with_untracked(&repo_dir).unwrap_or_default()
        };
        let diff_stat = if Self::is_test_session() {
            None
        } else {
            git_output_string(&repo_dir, &["diff", "--stat", "HEAD"])
                .ok()
                .map(|value| truncate_chars(&value, DIFF_STAT_LIMIT))
                .filter(|value| !value.trim().is_empty())
        };

        let mut record = build::SelfDevCustomizationRecord::new(id, goal, expected_behavior);
        record.intent = params.customization_intent;
        record.rationale = params.rationale;
        record.update_hints = params.update_hints.unwrap_or_default();
        record.provenance = build::SelfDevCustomizationProvenance {
            session_id: Some(ctx.session_id.clone()),
            repo_dir: Some(repo_dir.clone()),
            working_dir: ctx.working_dir.clone(),
            source: Some(source.clone()),
            touched_paths: touched_paths.into_iter().collect(),
            diff_stat,
            patch_path: None,
        };
        record.validation.commands = params.validation_commands.unwrap_or_default();
        record.build.source_fingerprint = Some(source.fingerprint.clone());

        let stored = build::create_customization_record(record, Some(&diff))?;
        let memory_id = write_customization_memory(&stored, &repo_dir, &ctx.session_id).ok();

        let mut output = format!(
            "Recorded self-dev customization `{}`.\n\nGoal: {}\nExpected behavior: {}\nStatus: active",
            stored.id, stored.goal, stored.expected_behavior
        );
        if let Some(path) = stored.provenance.patch_path.as_ref() {
            output.push_str(&format!("\nPatch: {}", path.display()));
        }
        if let Some(memory_id) = memory_id.as_deref() {
            output.push_str(&format!("\nMemory: {}", memory_id));
        }

        Ok(ToolOutput::new(output).with_metadata(json!({
            "id": stored.id,
            "status": stored.status,
            "record_path": build::customization_record_path(&stored.id)?.to_string_lossy(),
            "patch_path": stored.provenance.patch_path.as_ref().map(|p| p.to_string_lossy().to_string()),
            "memory_id": memory_id,
        })))
    }

    pub(super) async fn do_list_customizations(&self) -> Result<ToolOutput> {
        let records = build::list_customization_records()?;
        if records.is_empty() {
            return Ok(ToolOutput::new("No self-dev customizations recorded."));
        }

        let mut output = String::from("## Self-Dev Customizations\n\n");
        for record in &records {
            output.push_str(&format!(
                "- `{}` — {:?}\n  Goal: {}\n  Updated: {}\n",
                record.id, record.status, record.goal, record.updated_at
            ));
            if !record.provenance.touched_paths.is_empty() {
                output.push_str(&format!(
                    "  Paths: {}\n",
                    record.provenance.touched_paths.join(", ")
                ));
            }
        }

        Ok(ToolOutput::new(output).with_metadata(json!({ "count": records.len() })))
    }

    pub(super) async fn do_inspect_customization(&self, id: Option<String>) -> Result<ToolOutput> {
        let id = non_empty(id, "id")?;
        let Some(record) = build::load_customization_record(&id)? else {
            return Ok(ToolOutput::new(format!(
                "Self-dev customization `{}` was not found.",
                id
            )));
        };

        let mut output = format!(
            "## Self-Dev Customization `{}`\n\n**Status:** {:?}\n**Goal:** {}\n**Expected behavior:** {}\n**Created:** {}\n**Updated:** {}\n",
            record.id,
            record.status,
            record.goal,
            record.expected_behavior,
            record.created_at,
            record.updated_at
        );
        if let Some(rationale) = record.rationale.as_deref() {
            output.push_str(&format!("**Rationale:** {}\n", rationale));
        }
        if !record.update_hints.is_empty() {
            output.push_str(&format!(
                "**Update hints:** {}\n",
                record.update_hints.join("; ")
            ));
        }
        if !record.validation.commands.is_empty() {
            output.push_str(&format!(
                "**Validation:** `{}`\n",
                record.validation.commands.join("`, `")
            ));
        }
        if let Some(status) = record.validation.last_status.as_ref() {
            output.push_str(&format!("**Last validation:** {:?}\n", status));
        }
        if !record.provenance.touched_paths.is_empty() {
            output.push_str(&format!(
                "\n**Touched paths:** {}\n",
                record.provenance.touched_paths.join(", ")
            ));
        }
        if let Some(source) = record.provenance.source.as_ref() {
            output.push_str(&format!(
                "**Source:** `{}` dirty={} changed_paths={}\n",
                source.fingerprint, source.dirty, source.changed_paths
            ));
        }
        if let Some(patch) = record.provenance.patch_path.as_ref() {
            output.push_str(&format!("**Patch:** {}\n", patch.display()));
        }
        if !record.outcomes.is_empty() {
            output.push_str("\n## Outcomes\n\n");
            for outcome in &record.outcomes {
                output.push_str(&format!(
                    "- {:?} at {}{}\n",
                    outcome.status,
                    outcome.timestamp,
                    outcome
                        .detail
                        .as_deref()
                        .map(|detail| format!(" — {}", detail))
                        .unwrap_or_default()
                ));
            }
        }

        Ok(ToolOutput::new(output).with_metadata(json!({ "record": record })))
    }

    pub(super) async fn do_disable_customization(
        &self,
        id: Option<String>,
        reason: Option<String>,
        ctx: &ToolContext,
    ) -> Result<ToolOutput> {
        let id = non_empty(id, "id")?;
        let record = build::disable_customization_record(&id, reason)?;
        let memory_removed = forget_customization_memory(&record, ctx).unwrap_or(false);
        Ok(ToolOutput::new(format!(
            "Disabled self-dev customization `{}`.{}",
            record.id,
            if memory_removed {
                " Removed compact memory entry."
            } else {
                ""
            }
        )))
    }
}

fn customization_memory_id(record_id: &str) -> String {
    format!("selfdev-customization-{}", record_id)
}

fn write_customization_memory(
    record: &build::SelfDevCustomizationRecord,
    repo_dir: &Path,
    session_id: &str,
) -> Result<String> {
    let paths = if record.provenance.touched_paths.is_empty() {
        "no touched paths recorded".to_string()
    } else {
        record.provenance.touched_paths.join(", ")
    };
    let validation = if record.validation.commands.is_empty() {
        "no validation command recorded".to_string()
    } else {
        record.validation.commands.join("; ")
    };
    let content = format!(
        "Self-dev customization `{}` is active. Goal: {}. Expected behavior: {}. Paths: {}. Validation: {}.",
        record.id, record.goal, record.expected_behavior, paths, validation
    );
    let mut entry = MemoryEntry::new(
        MemoryCategory::Custom("self_dev_customization".to_string()),
        content,
    )
    .with_source(session_id.to_string())
    .with_tags(memory_tags(record));
    entry.id = customization_memory_id(&record.id);
    entry.trust = TrustLevel::High;
    entry.refresh_search_text();

    MemoryManager::new()
        .with_project_dir(repo_dir)
        .upsert_project_memory(entry)
}

fn forget_customization_memory(
    record: &build::SelfDevCustomizationRecord,
    ctx: &ToolContext,
) -> Result<bool> {
    let manager = record
        .provenance
        .repo_dir
        .as_ref()
        .cloned()
        .or_else(|| SelfDevTool::resolve_repo_dir(ctx.working_dir.as_deref()))
        .map(|dir| MemoryManager::new().with_project_dir(dir))
        .unwrap_or_else(MemoryManager::new);
    manager.forget(&customization_memory_id(&record.id))
}

fn memory_tags(record: &build::SelfDevCustomizationRecord) -> Vec<String> {
    let mut tags = vec![
        "selfdev".to_string(),
        format!("customization:{}", record.id),
        "status:active".to_string(),
    ];
    tags.extend(
        record
            .provenance
            .touched_paths
            .iter()
            .take(8)
            .map(|path| format!("path:{}", path)),
    );
    tags
}

fn non_empty(value: Option<String>, field: &str) -> Result<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("{} required", field))
}

fn detected_touched_paths(repo_dir: &Path) -> Result<BTreeSet<String>> {
    if SelfDevTool::is_test_session() {
        return Ok(BTreeSet::new());
    }
    let status = git_output_string(repo_dir, &["status", "--porcelain=v1"])?;
    let mut paths = BTreeSet::new();
    for line in status.lines() {
        let raw = line
            .get(3..)
            .unwrap_or(line)
            .trim()
            .rsplit_once(" -> ")
            .map(|(_, new_path)| new_path)
            .unwrap_or_else(|| line.get(3..).unwrap_or(line).trim());
        if !raw.is_empty() {
            paths.insert(raw.to_string());
        }
    }
    Ok(paths)
}

fn git_output_string(repo_dir: &Path, args: &[&str]) -> Result<String> {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(repo_dir)
        .output()?;
    if !output.status.success() {
        anyhow::bail!("git {} failed", args.join(" "));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        value.to_string()
    } else {
        let mut truncated: String = value.chars().take(max_chars).collect();
        truncated.push_str("\n...");
        truncated
    }
}
