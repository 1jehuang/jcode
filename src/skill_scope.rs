use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const POLICY_RELATIVE_PATH: &str = ".jcode/skills.scope.json";

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SkillScopeState {
    Visible,
    Discoverable,
    Blocked,
}

impl SkillScopeState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Visible => "visible",
            Self::Discoverable => "discoverable",
            Self::Blocked => "blocked",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillScopeEntry {
    pub name: String,
    pub state: SkillScopeState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillScopePolicy {
    pub version: u32,
    pub default_state: SkillScopeState,
    pub skills: Vec<SkillScopeEntry>,
}

impl Default for SkillScopePolicy {
    fn default() -> Self {
        Self {
            version: 1,
            default_state: SkillScopeState::Visible,
            skills: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillScopeReport {
    pub policy_path: String,
    pub exists: bool,
    pub created: bool,
    pub updated: bool,
    pub policy: SkillScopePolicy,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillScopeDecision {
    pub name: String,
    pub state: SkillScopeState,
    pub explicit: bool,
    pub selected: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillScopeSelection {
    pub policy_path: String,
    pub policy_exists: bool,
    pub selected: Vec<SkillScopeDecision>,
    pub skipped: Vec<SkillScopeDecision>,
}

impl SkillScopeSelection {
    pub fn selected_names(&self) -> Vec<String> {
        self.selected
            .iter()
            .map(|decision| decision.name.clone())
            .collect()
    }
}

pub fn policy_path(root: &Path) -> PathBuf {
    root.join(POLICY_RELATIVE_PATH)
}

pub fn init_policy(root: &Path, force: bool) -> Result<SkillScopeReport> {
    let path = policy_path(root);
    let exists_before = path.exists();
    if exists_before && !force {
        return Ok(SkillScopeReport {
            policy_path: path.display().to_string(),
            exists: true,
            created: false,
            updated: false,
            policy: load_policy(root)?.unwrap_or_default(),
        });
    }

    let policy = SkillScopePolicy::default();
    write_policy(root, &policy)?;
    Ok(SkillScopeReport {
        policy_path: path.display().to_string(),
        exists: true,
        created: !exists_before,
        updated: exists_before,
        policy,
    })
}

pub fn list_policy(root: &Path) -> Result<SkillScopeReport> {
    let path = policy_path(root);
    let exists = path.exists();
    Ok(SkillScopeReport {
        policy_path: path.display().to_string(),
        exists,
        created: false,
        updated: false,
        policy: load_policy(root)?.unwrap_or_default(),
    })
}

pub fn set_skill_state(
    root: &Path,
    name: &str,
    state: SkillScopeState,
    reason: Option<String>,
) -> Result<SkillScopeReport> {
    validate_skill_name(name)?;
    let mut policy = load_policy(root)?.unwrap_or_default();
    policy.version = 1;
    policy.default_state = SkillScopeState::Visible;
    if let Some(entry) = policy.skills.iter_mut().find(|entry| entry.name == name) {
        entry.state = state;
        entry.reason = normalize_reason(reason);
    } else {
        policy.skills.push(SkillScopeEntry {
            name: name.to_string(),
            state,
            reason: normalize_reason(reason),
        });
    }
    policy.skills.sort_by(|a, b| a.name.cmp(&b.name));
    write_policy(root, &policy)?;
    let path = policy_path(root);
    Ok(SkillScopeReport {
        policy_path: path.display().to_string(),
        exists: true,
        created: false,
        updated: true,
        policy,
    })
}

pub fn load_policy(root: &Path) -> Result<Option<SkillScopePolicy>> {
    let path = policy_path(root);
    if !path.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(&path)?;
    let mut policy: SkillScopePolicy = serde_json::from_str(&content)?;
    policy.skills.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(Some(policy))
}

pub fn apply_policy_for_selection(
    root: &Path,
    selected: Vec<String>,
    explicit: &[String],
) -> Result<SkillScopeSelection> {
    let path = policy_path(root);
    let policy = load_policy(root)?;
    let policy_exists = policy.is_some();
    let policy = policy.unwrap_or_default();
    let explicit_set = explicit
        .iter()
        .map(|name| (name.as_str(), ()))
        .collect::<BTreeMap<_, _>>();
    let scoped = policy
        .skills
        .iter()
        .map(|entry| (entry.name.as_str(), entry))
        .collect::<BTreeMap<_, _>>();

    let mut kept = Vec::new();
    let mut skipped = Vec::new();

    for name in selected {
        let explicit = explicit_set.contains_key(name.as_str());
        let entry = scoped.get(name.as_str()).copied();
        let state = entry
            .map(|entry| entry.state)
            .unwrap_or(policy.default_state);
        let policy_reason = entry.and_then(|entry| entry.reason.clone());
        let skip_reason = match state {
            SkillScopeState::Visible => None,
            SkillScopeState::Discoverable if explicit => None,
            SkillScopeState::Discoverable => Some(
                "state is discoverable, so automatic routing is disabled unless explicitly requested"
                    .to_string(),
            ),
            SkillScopeState::Blocked => Some("state is blocked by project skill scope policy".to_string()),
        };
        let selected = skip_reason.is_none();
        let decision = SkillScopeDecision {
            name,
            state,
            explicit,
            selected,
            reason: skip_reason,
            policy_reason,
        };
        if selected {
            kept.push(decision);
        } else {
            skipped.push(decision);
        }
    }

    Ok(SkillScopeSelection {
        policy_path: path.display().to_string(),
        policy_exists,
        selected: kept,
        skipped,
    })
}

pub fn write_policy(root: &Path, policy: &SkillScopePolicy) -> Result<()> {
    let path = policy_path(root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = serde_json::to_string_pretty(policy)?;
    std::fs::write(path, format!("{content}\n"))?;
    Ok(())
}

fn normalize_reason(reason: Option<String>) -> Option<String> {
    reason
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn validate_skill_name(name: &str) -> Result<()> {
    let valid = !name.starts_with('.')
        && !name.is_empty()
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains("..")
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'));
    if !valid {
        anyhow::bail!(
            "skill scope names must use letters, numbers, dots, underscores, or hyphens: {name}"
        );
    }
    Ok(())
}
