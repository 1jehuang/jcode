//! Explicit, per-session snapshots of the instruction sources Jcode can load.
//!
//! Only known instruction locations are discoverable. IDs identify files, not
//! categories, so opting into one file never opts into another appearing later.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstructionSourceKind {
    AgentsMd,
    SystemPrompt,
    PromptOverlay,
    PreferredTools,
    SwarmPrompt,
    SessionSystemPrompt,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstructionSourceSnapshot {
    pub id: String,
    pub kind: InstructionSourceKind,
    pub label: String,
    pub path: Option<PathBuf>,
    pub content: String,
    /// Project files precede global files even when a target disappears or a
    /// symlink changes and only the retained snapshot remains discoverable.
    #[serde(default)]
    pub priority: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InstructionSourceChoice {
    pub id: String,
    pub kind: InstructionSourceKind,
    pub label: String,
    pub path: Option<PathBuf>,
    pub available: bool,
    pub included: bool,
}

struct Candidate {
    id: String,
    kind: InstructionSourceKind,
    label: String,
    path: Option<PathBuf>,
    content: Option<String>,
}

fn candidates(working_dir: Option<&Path>, system_prompt: Option<&str>) -> Vec<Candidate> {
    use InstructionSourceKind::*;
    let mut files = Vec::new();
    let project = working_dir.unwrap_or(Path::new("."));
    files.push((
        project.join("AGENTS.md"),
        AgentsMd,
        "Project Instructions (AGENTS.md)",
    ));
    if let Ok(path) = crate::storage::user_home_path("AGENTS.md") {
        files.push((path, AgentsMd, "Global Instructions (~/AGENTS.md)"));
    }
    for (name, kind, project_label, global_label) in [
        (
            "system-prompt.md",
            SystemPrompt,
            "Project System Prompt (.jcode/system-prompt.md)",
            "Global System Prompt (~/.jcode/system-prompt.md)",
        ),
        (
            "prompt-overlay.md",
            PromptOverlay,
            "Project Prompt Overlay (.jcode/prompt-overlay.md)",
            "Global Prompt Overlay (~/.jcode/prompt-overlay.md)",
        ),
        (
            "preferred-tools.md",
            PreferredTools,
            "Project Preferred Tools (.jcode/preferred-tools.md)",
            "Global Preferred Tools (~/.jcode/preferred-tools.md)",
        ),
        (
            "swarm-prompt.md",
            SwarmPrompt,
            "Project Swarm Prompt (.jcode/swarm-prompt.md)",
            "Global Swarm Prompt (~/.jcode/swarm-prompt.md)",
        ),
    ] {
        files.push((project.join(".jcode").join(name), kind, project_label));
        if let Ok(home) = crate::storage::jcode_dir() {
            files.push((home.join(name), kind, global_label));
        }
    }
    let mut seen = BTreeSet::new();
    let mut sources = Vec::new();
    for (path, kind, label) in files {
        let path = std::fs::canonicalize(&path).unwrap_or_else(|_| {
            if path.is_absolute() {
                path.clone()
            } else {
                std::env::current_dir().unwrap_or_default().join(&path)
            }
        });
        let id = format!("file:{}", path.display());
        if !seen.insert(id.clone()) {
            continue;
        }
        let content = std::fs::read_to_string(&path)
            .ok()
            .filter(|text| !text.trim().is_empty());
        sources.push(Candidate {
            id,
            kind,
            label: label.into(),
            path: Some(path),
            content,
        });
    }
    if let Some(content) = system_prompt {
        sources.push(Candidate {
            id: "session:system_prompt".into(),
            kind: SessionSystemPrompt,
            label: "Stored session instructions (additive)".into(),
            path: None,
            content: Some(content.into()),
        });
    }
    sources
}

/// List individual known files, including unavailable retained snapshots.
/// No file body is returned to clients by this metadata API.
pub fn choices(
    working_dir: Option<&Path>,
    system_prompt: Option<&str>,
    selected: &[InstructionSourceSnapshot],
) -> Vec<InstructionSourceChoice> {
    let mut choices: Vec<_> = candidates(working_dir, system_prompt)
        .into_iter()
        .map(|source| {
            let saved = selected.iter().find(|saved| saved.id == source.id);
            InstructionSourceChoice {
                included: saved.is_some(),
                available: source.content.is_some(),
                id: source.id,
                kind: saved.map_or(source.kind, |saved| saved.kind),
                label: saved.map_or(source.label, |saved| saved.label.clone()),
                path: source.path,
            }
        })
        .collect();
    for source in selected {
        if !choices.iter().any(|choice| choice.id == source.id) {
            choices.push(InstructionSourceChoice {
                id: source.id.clone(),
                kind: source.kind,
                label: source.label.clone(),
                path: source.path.clone(),
                available: false,
                included: true,
            });
        }
    }
    choices
}

/// Resolve a strict allowlist against server-discovered sources before mutation.
/// Available files are refreshed when options are applied. Missing previously
/// selected files can retain their snapshot, but cannot be newly included.
pub fn select(
    working_dir: Option<&Path>,
    system_prompt: Option<&str>,
    selected: &[InstructionSourceSnapshot],
    encoded_ids: &str,
) -> Result<Vec<InstructionSourceSnapshot>> {
    let ids: Vec<String> = serde_json::from_str(encoded_ids)
        .context("Instruction sources must be a JSON array of source IDs")?;
    let requested: BTreeSet<_> = ids.iter().collect();
    if requested.len() != ids.len() {
        bail!("Instruction source IDs must not be repeated");
    }
    let candidates = candidates(working_dir, system_prompt);
    let mut result = Vec::new();
    // Discovery order keeps project-over-global replacement precedence stable,
    // independently of the order of checkboxes in a client request.
    for source in &candidates {
        if !requested.contains(&source.id) {
            continue;
        }
        if let Some(content) = &source.content {
            let mut snapshot = selected
                .iter()
                .find(|saved| saved.id == source.id)
                .cloned()
                .unwrap_or_else(|| InstructionSourceSnapshot {
                    id: source.id.clone(),
                    kind: source.kind,
                    label: source.label.clone(),
                    path: source.path.clone(),
                    content: String::new(),
                    priority: if source.kind == InstructionSourceKind::SessionSystemPrompt {
                        2
                    } else if source.label.starts_with("Global ") {
                        1
                    } else {
                        0
                    },
                });
            // A new symlink alias must not silently change an opted-in file's
            // role or its project/global replacement precedence.
            snapshot.content = content.clone();
            result.push(snapshot);
        } else if let Some(saved) = selected.iter().find(|saved| saved.id == source.id) {
            result.push(saved.clone());
        } else {
            bail!("Instruction source is unavailable: {}", source.label);
        }
    }
    for id in ids {
        if result.iter().any(|source| source.id == id) {
            continue;
        }
        if let Some(saved) = selected.iter().find(|saved| saved.id == id) {
            result.push(saved.clone());
        } else {
            bail!("Unknown instruction source ID: {id}");
        }
    }
    result.sort_by_key(|source| source.priority);
    Ok(result)
}

pub fn first_content(
    sources: &[InstructionSourceSnapshot],
    kind: InstructionSourceKind,
) -> Option<&str> {
    sources
        .iter()
        .find(|source| source.kind == kind)
        .map(|source| source.content.as_str())
}

pub fn formatted_content(
    sources: &[InstructionSourceSnapshot],
    kind: InstructionSourceKind,
) -> (Option<String>, usize) {
    let selected: Vec<_> = sources
        .iter()
        .filter(|source| source.kind == kind)
        .collect();
    let chars = selected.iter().map(|source| source.content.len()).sum();
    let content = (!selected.is_empty()).then(|| {
        selected
            .iter()
            .map(|source| format!("# {}\n\n{}", source.label, source.content.trim()))
            .collect::<Vec<_>>()
            .join("\n\n")
    });
    (content, chars)
}

#[cfg(test)]
#[path = "instruction_sources_tests.rs"]
mod tests;
