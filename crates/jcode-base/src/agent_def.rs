//! Agent definition files.
//!
//! A definition is a Markdown file with YAML frontmatter, compatible with
//! Claude Code's `.claude/agents/*.md`. The body is the persona. Swarm `spawn`
//! can name a definition with `agent`, and the worker receives the persona as a
//! hidden message at the start of its transcript (never a system prompt swap,
//! so prompt caching stays stable).
//!
//! Locations, first match by `name` wins:
//!
//! 1. `<project>/.jcode/agents`
//! 2. `<project>/.claude/agents`
//! 3. `~/.jcode/agents`
//! 4. `~/.claude/agents`
//!
//! Project directories are scanned from the session working directory up to
//! the project root, closest first. Scans are recursive and identity comes from
//! `name`, not the file name. A file without `name` is documentation and is
//! skipped silently. A missing `description` or invalid YAML skips the file
//! with one warning. Unknown frontmatter keys are ignored.

mod tool_names;

use serde::Deserialize;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

pub use tool_names::{map_claude_tool, map_claude_tools};

/// Longest accepted definition name.
const MAX_NAME_LEN: usize = 256;

/// Where a definition came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefinitionOrigin {
    /// A project directory. Its policy fields need project trust.
    Project,
    /// A user directory. Always trusted.
    User,
}

/// The policy fields of a definition: what it asks the operator to apply.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DefinitionPolicy {
    pub model: Option<String>,
    pub effort: Option<String>,
    /// Allowlist, already mapped to jcode tool names. `None` means no list.
    pub tools: Option<Vec<String>>,
    /// Denylist, already mapped to jcode tool names.
    pub disallowed_tools: Vec<String>,
}

impl DefinitionPolicy {
    pub fn is_empty(&self) -> bool {
        self.model.is_none()
            && self.effort.is_none()
            && self.tools.is_none()
            && self.disallowed_tools.is_empty()
    }
}

/// A loaded definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentDefinition {
    pub name: String,
    pub description: String,
    /// The persona: the Markdown body after the frontmatter.
    pub persona: String,
    pub policy: DefinitionPolicy,
    pub origin: DefinitionOrigin,
    pub path: PathBuf,
}

#[derive(Debug, Deserialize)]
struct Frontmatter {
    name: Option<String>,
    description: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    tools: Option<ListOrString>,
    #[serde(default, alias = "disallowed-tools", rename = "disallowedTools")]
    disallowed_tools: Option<ListOrString>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ListOrString {
    Text(String),
    List(Vec<String>),
}

impl ListOrString {
    fn into_entries(self) -> Vec<String> {
        match self {
            // A comma separated string, but keep commas inside parentheses.
            Self::Text(text) => split_top_level_commas(&text),
            Self::List(items) => items,
        }
        .into_iter()
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
        .collect()
    }
}

fn split_top_level_commas(text: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut current = String::new();
    for ch in text.chars() {
        match ch {
            '(' => {
                depth += 1;
                current.push(ch);
            }
            ')' => {
                depth = depth.saturating_sub(1);
                current.push(ch);
            }
            ',' if depth == 0 => parts.push(std::mem::take(&mut current)),
            _ => current.push(ch),
        }
    }
    parts.push(current);
    parts
}

/// Names are `[a-z0-9_-]`, at most 256 chars, without `:`.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME_LEN
        && name
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_' || ch == '-')
}

/// Outcome of reading one file.
#[derive(Debug, PartialEq, Eq)]
enum Parsed {
    Definition(Box<AgentDefinition>),
    /// No `name`: documentation, skipped silently.
    NotADefinition,
    /// Skipped with a warning.
    Invalid(String),
}

fn parse_file(path: &Path, origin: DefinitionOrigin) -> Parsed {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Parsed::Invalid("cannot read file".to_string());
    };
    let trimmed = text.trim_start_matches('\u{feff}').trim_start();
    let Some(rest) = trimmed.strip_prefix("---") else {
        return Parsed::NotADefinition;
    };
    let rest = rest.trim_start_matches(['\r', '\n']);
    let Some(end) = rest.find("\n---") else {
        return Parsed::Invalid("unclosed frontmatter".to_string());
    };
    let (yaml, after) = rest.split_at(end);
    let body = after[4..]
        .trim_start_matches(['\r', '\n'])
        .trim()
        .to_string();
    let front: Frontmatter = match serde_yaml::from_str(yaml) {
        Ok(front) => front,
        Err(error) => return Parsed::Invalid(format!("invalid frontmatter: {error}")),
    };
    let Some(name) = front
        .name
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
    else {
        return Parsed::NotADefinition;
    };
    if !valid_name(&name) {
        return Parsed::Invalid(format!(
            "invalid name '{name}' (use a-z, 0-9, '-' and '_', at most {MAX_NAME_LEN} chars)"
        ));
    }
    let Some(description) = front
        .description
        .map(|d| d.trim().to_string())
        .filter(|d| !d.is_empty())
    else {
        return Parsed::Invalid("missing description".to_string());
    };
    let tools = front
        .tools
        .map(|list| map_claude_tools(&list.into_entries()));
    let disallowed_tools = front
        .disallowed_tools
        .map(|list| map_claude_tools(&list.into_entries()))
        .unwrap_or_default();
    let policy = DefinitionPolicy {
        model: front
            .model
            .map(|m| m.trim().to_string())
            .filter(|m| !m.is_empty()),
        effort: front
            .effort
            .map(|e| e.trim().to_ascii_lowercase())
            .filter(|e| !e.is_empty()),
        tools,
        disallowed_tools,
    };
    Parsed::Definition(Box::new(AgentDefinition {
        name,
        description,
        persona: body,
        policy,
        origin,
        path: path.to_path_buf(),
    }))
}

static WARNED: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

fn warn_once(path: &Path, reason: &str) {
    let first = WARNED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(format!("{}:{reason}", path.display()));
    if first {
        crate::logging::warn(&format!(
            "Skipping agent definition {}: {reason}",
            path.display()
        ));
    }
}

/// Recursively collect definitions under `dir`, in sorted path order.
fn scan_dir(dir: &Path, origin: DefinitionOrigin, out: &mut Vec<AgentDefinition>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            if !path.is_symlink() {
                scan_dir(&path, origin, out);
            }
        } else if path.extension().is_some_and(|ext| ext == "md") {
            match parse_file(&path, origin) {
                Parsed::Definition(definition) => out.push(*definition),
                Parsed::NotADefinition => {}
                Parsed::Invalid(reason) => warn_once(&path, &reason),
            }
        }
    }
}

/// Directories to scan, highest priority first.
fn search_dirs(working_dir: Option<&Path>) -> Vec<(PathBuf, DefinitionOrigin)> {
    let mut dirs = Vec::new();
    if let Some(cwd) = working_dir {
        let start = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
        // Walk up to the project root: the nearest ancestor with a `.git`.
        for dir in start.ancestors() {
            for sub in [".jcode/agents", ".claude/agents"] {
                dirs.push((dir.join(sub), DefinitionOrigin::Project));
            }
            if dir.join(".git").exists() {
                break;
            }
        }
    }
    if let Ok(jcode) = crate::storage::jcode_dir() {
        dirs.push((jcode.join("agents"), DefinitionOrigin::User));
    }
    if let Ok(claude) = crate::storage::user_home_path(".claude/agents") {
        dirs.push((claude, DefinitionOrigin::User));
    }
    dirs
}

/// Load the definitions visible from `working_dir`. The first definition for a
/// name wins, so project beats user and `.jcode` beats `.claude`. The result is
/// sorted by name so listings are stable.
pub fn load_agent_definitions(working_dir: Option<&Path>) -> BTreeMap<String, AgentDefinition> {
    let mut found: BTreeMap<String, AgentDefinition> = BTreeMap::new();
    for (dir, origin) in search_dirs(working_dir) {
        let mut batch = Vec::new();
        scan_dir(&dir, origin, &mut batch);
        for definition in batch {
            found.entry(definition.name.clone()).or_insert(definition);
        }
    }
    found
}

/// Look up one definition by name.
pub fn find_agent_definition(name: &str, working_dir: Option<&Path>) -> Option<AgentDefinition> {
    load_agent_definitions(working_dir).remove(name.trim())
}

/// Names and descriptions, for the swarm tool schema.
pub fn agent_definition_summaries(working_dir: Option<&Path>) -> Vec<(String, String)> {
    load_agent_definitions(working_dir)
        .into_values()
        .map(|definition| (definition.name, definition.description))
        .collect()
}

#[cfg(test)]
#[path = "agent_def_tests.rs"]
mod tests;
