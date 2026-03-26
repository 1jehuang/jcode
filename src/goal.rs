use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum GoalScope {
    Global,
    #[default]
    Project,
}

impl GoalScope {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "global" => Some(Self::Global),
            "project" => Some(Self::Project),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Project => "project",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum GoalStatus {
    Draft,
    #[default]
    Active,
    Paused,
    Blocked,
    Completed,
    Archived,
    Abandoned,
}

impl GoalStatus {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "draft" => Some(Self::Draft),
            "active" => Some(Self::Active),
            "paused" => Some(Self::Paused),
            "blocked" => Some(Self::Blocked),
            "completed" => Some(Self::Completed),
            "archived" => Some(Self::Archived),
            "abandoned" => Some(Self::Abandoned),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Blocked => "blocked",
            Self::Completed => "completed",
            Self::Archived => "archived",
            Self::Abandoned => "abandoned",
        }
    }

    fn sort_rank(self) -> u8 {
        match self {
            Self::Active => 0,
            Self::Blocked => 1,
            Self::Draft => 2,
            Self::Paused => 3,
            Self::Completed => 4,
            Self::Archived => 5,
            Self::Abandoned => 6,
        }
    }

    pub fn is_resumable(self) -> bool {
        matches!(self, Self::Active | Self::Blocked | Self::Draft)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct GoalStep {
    pub id: String,
    pub content: String,
    #[serde(default = "default_pending_status")]
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct GoalMilestone {
    pub id: String,
    pub title: String,
    #[serde(default = "default_pending_status")]
    pub status: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<GoalStep>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GoalUpdate {
    pub at: DateTime<Utc>,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Goal {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub scope: GoalScope,
    #[serde(default)]
    pub status: GoalStatus,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub why: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub success_criteria: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub milestones: Vec<GoalMilestone>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub next_steps: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blockers: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_milestone_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress_percent: Option<u8>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub updates: Vec<GoalUpdate>,
}

impl Goal {
    pub fn new(title: &str, scope: GoalScope) -> Self {
        let now = Utc::now();
        let trimmed = title.trim();
        Self {
            id: slugify(trimmed),
            title: trimmed.to_string(),
            scope,
            status: GoalStatus::Active,
            description: String::new(),
            why: String::new(),
            success_criteria: Vec::new(),
            milestones: Vec::new(),
            next_steps: Vec::new(),
            blockers: Vec::new(),
            current_milestone_id: None,
            progress_percent: None,
            created_at: now,
            updated_at: now,
            updates: Vec::new(),
        }
    }

    pub fn current_milestone(&self) -> Option<&GoalMilestone> {
        let current_id = self.current_milestone_id.as_deref()?;
        self.milestones.iter().find(|m| m.id == current_id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoalDisplayMode {
    Auto,
    Focus,
    UpdateOnly,
    None,
}

impl GoalDisplayMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(Self::Auto),
            "focus" => Some(Self::Focus),
            "update_only" => Some(Self::UpdateOnly),
            "none" => Some(Self::None),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct GoalCreateInput {
    pub id: Option<String>,
    pub title: String,
    pub scope: GoalScope,
    pub description: Option<String>,
    pub why: Option<String>,
    pub success_criteria: Vec<String>,
    pub milestones: Vec<GoalMilestone>,
    pub next_steps: Vec<String>,
    pub blockers: Vec<String>,
    pub current_milestone_id: Option<String>,
    pub progress_percent: Option<u8>,
}

#[derive(Debug, Clone, Default)]
pub struct GoalUpdateInput {
    pub title: Option<String>,
    pub description: Option<String>,
    pub why: Option<String>,
    pub status: Option<GoalStatus>,
    pub success_criteria: Option<Vec<String>>,
    pub milestones: Option<Vec<GoalMilestone>>,
    pub next_steps: Option<Vec<String>>,
    pub blockers: Option<Vec<String>>,
    pub current_milestone_id: Option<Option<String>>,
    pub progress_percent: Option<Option<u8>>,
    pub checkpoint_summary: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct GoalAttachment {
    goal_id: String,
    scope: GoalScope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    project_hash: Option<String>,
    title: String,
    attached_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct GoalDisplayResult {
    pub goal: Goal,
    pub snapshot: crate::side_panel::SidePanelSnapshot,
}

fn default_pending_status() -> String {
    "pending".to_string()
}

pub fn create_goal(input: GoalCreateInput, working_dir: Option<&Path>) -> Result<Goal> {
    if input.title.trim().is_empty() {
        anyhow::bail!("goal title cannot be empty");
    }
    let mut goal = Goal::new(&input.title, input.scope);
    if let Some(id) = input.id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        goal.id = sanitize_id(id);
    }
    goal.id = next_available_goal_id(&goal.id, goal.scope, working_dir)?;
    goal.description = input.description.unwrap_or_default().trim().to_string();
    goal.why = input.why.unwrap_or_default().trim().to_string();
    goal.success_criteria = trim_vec(input.success_criteria);
    goal.milestones = input.milestones;
    goal.next_steps = trim_vec(input.next_steps);
    goal.blockers = trim_vec(input.blockers);
    goal.current_milestone_id = input.current_milestone_id;
    goal.progress_percent = input.progress_percent.map(|p| p.min(100));
    goal.updated_at = Utc::now();
    save_goal(&goal, working_dir)?;
    sync_goal_memory(&goal, working_dir)?;
    Ok(goal)
}

pub fn update_goal(
    id: &str,
    scope_hint: Option<GoalScope>,
    working_dir: Option<&Path>,
    update: GoalUpdateInput,
) -> Result<Option<Goal>> {
    let Some(mut goal) = load_goal(id, scope_hint, working_dir)? else {
        return Ok(None);
    };

    if let Some(title) = update
        .title
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        goal.title = title.to_string();
    }
    if let Some(description) = update.description {
        goal.description = description.trim().to_string();
    }
    if let Some(why) = update.why {
        goal.why = why.trim().to_string();
    }
    if let Some(status) = update.status {
        goal.status = status;
    }
    if let Some(criteria) = update.success_criteria {
        goal.success_criteria = trim_vec(criteria);
    }
    if let Some(milestones) = update.milestones {
        goal.milestones = milestones;
    }
    if let Some(next_steps) = update.next_steps {
        goal.next_steps = trim_vec(next_steps);
    }
    if let Some(blockers) = update.blockers {
        goal.blockers = trim_vec(blockers);
    }
    if let Some(current_milestone_id) = update.current_milestone_id {
        goal.current_milestone_id = current_milestone_id.map(|s| s.trim().to_string());
    }
    if let Some(progress_percent) = update.progress_percent {
        goal.progress_percent = progress_percent.map(|p| p.min(100));
    }
    if let Some(summary) = update
        .checkpoint_summary
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        goal.updates.push(GoalUpdate {
            at: Utc::now(),
            summary: summary.to_string(),
        });
    }
    goal.updated_at = Utc::now();
    save_goal(&goal, working_dir)?;
    sync_goal_memory(&goal, working_dir)?;
    Ok(Some(goal))
}

pub fn load_goal(
    id: &str,
    scope_hint: Option<GoalScope>,
    working_dir: Option<&Path>,
) -> Result<Option<Goal>> {
    let id = sanitize_id(id);
    let mut candidates = Vec::new();
    match scope_hint {
        Some(GoalScope::Global) => candidates.push(goal_file_in_dir(&global_goals_dir()?, &id)),
        Some(GoalScope::Project) => {
            if let Some(dir) = project_goals_dir(working_dir)? {
                candidates.push(goal_file_in_dir(&dir, &id));
            }
        }
        None => {
            if let Some(dir) = project_goals_dir(working_dir)? {
                candidates.push(goal_file_in_dir(&dir, &id));
            }
            candidates.push(goal_file_in_dir(&global_goals_dir()?, &id));
        }
    }

    for path in candidates {
        if path.exists() {
            let goal: Goal = crate::storage::read_json(&path)
                .with_context(|| format!("failed to read goal {}", path.display()))?;
            return Ok(Some(goal));
        }
    }
    Ok(None)
}

pub fn list_relevant_goals(working_dir: Option<&Path>) -> Result<Vec<Goal>> {
    let mut goals = load_goals_in_dir(&global_goals_dir()?)?;
    if let Some(project_dir) = project_goals_dir(working_dir)? {
        goals.extend(load_goals_in_dir(&project_dir)?);
    }
    sort_goals(&mut goals);
    Ok(goals)
}

pub fn resume_goal(session_id: &str, working_dir: Option<&Path>) -> Result<Option<Goal>> {
    if let Some(goal) = load_attached_goal(session_id, working_dir)? {
        if goal.status.is_resumable() {
            return Ok(Some(goal));
        }
    }

    let mut goals = list_relevant_goals(working_dir)?;
    goals.retain(|goal| goal.status.is_resumable());
    Ok(goals.into_iter().next())
}

pub fn attach_goal_to_session(
    session_id: &str,
    goal: &Goal,
    working_dir: Option<&Path>,
) -> Result<()> {
    let attachment = GoalAttachment {
        goal_id: goal.id.clone(),
        scope: goal.scope,
        project_hash: if goal.scope == GoalScope::Project {
            Some(project_hash(working_dir.ok_or_else(|| {
                anyhow::anyhow!("working_dir required for project goal")
            })?))
        } else {
            None
        },
        title: goal.title.clone(),
        attached_at: Utc::now(),
    };
    let path = session_attachment_path(session_id)?;
    crate::storage::write_json_fast(&path, &attachment)
}

pub fn load_attached_goal(session_id: &str, working_dir: Option<&Path>) -> Result<Option<Goal>> {
    let path = session_attachment_path(session_id)?;
    if !path.exists() {
        return Ok(None);
    }
    let attachment: GoalAttachment = crate::storage::read_json(&path)?;
    if attachment.scope == GoalScope::Project {
        let Some(dir) = working_dir else {
            return Ok(None);
        };
        let current_hash = project_hash(dir);
        if attachment.project_hash.as_deref() != Some(current_hash.as_str()) {
            return Ok(None);
        }
    }
    load_goal(&attachment.goal_id, Some(attachment.scope), working_dir)
}

pub fn open_goals_overview_for_session(
    session_id: &str,
    working_dir: Option<&Path>,
    focus: bool,
) -> Result<crate::side_panel::SidePanelSnapshot> {
    let goals = list_relevant_goals(working_dir)?;
    crate::side_panel::write_markdown_page(
        session_id,
        "goals",
        Some("Goals"),
        &render_goals_overview(&goals),
        focus,
    )
}

pub fn refresh_goals_overview_for_session(
    session_id: &str,
    working_dir: Option<&Path>,
) -> Result<Option<crate::side_panel::SidePanelSnapshot>> {
    let snapshot = crate::side_panel::snapshot_for_session(session_id)?;
    if !snapshot.pages.iter().any(|page| page.id == "goals") {
        return Ok(None);
    }

    let focus = snapshot.focused_page_id.as_deref() == Some("goals");
    Ok(Some(open_goals_overview_for_session(
        session_id,
        working_dir,
        focus,
    )?))
}

pub fn open_goal_for_session(
    session_id: &str,
    working_dir: Option<&Path>,
    id: &str,
    explicit_focus: bool,
) -> Result<Option<GoalDisplayResult>> {
    let Some(goal) = load_goal(id, None, working_dir)? else {
        return Ok(None);
    };
    let snapshot = write_goal_page(
        session_id,
        working_dir,
        &goal,
        if explicit_focus {
            GoalDisplayMode::Focus
        } else {
            GoalDisplayMode::Auto
        },
    )?;
    Ok(Some(GoalDisplayResult { goal, snapshot }))
}

pub fn resume_goal_for_session(
    session_id: &str,
    working_dir: Option<&Path>,
    explicit_focus: bool,
) -> Result<Option<GoalDisplayResult>> {
    let Some(goal) = resume_goal(session_id, working_dir)? else {
        return Ok(None);
    };
    let snapshot = write_goal_page(
        session_id,
        working_dir,
        &goal,
        if explicit_focus {
            GoalDisplayMode::Focus
        } else {
            GoalDisplayMode::Auto
        },
    )?;
    Ok(Some(GoalDisplayResult { goal, snapshot }))
}

pub fn write_goal_page(
    session_id: &str,
    working_dir: Option<&Path>,
    goal: &Goal,
    display: GoalDisplayMode,
) -> Result<crate::side_panel::SidePanelSnapshot> {
    let page_id = goal_page_id(&goal.id);
    let page_title = format!("Goal: {}", goal.title);
    let focus = match display {
        GoalDisplayMode::None => false,
        GoalDisplayMode::Focus => true,
        GoalDisplayMode::UpdateOnly => false,
        GoalDisplayMode::Auto => should_focus_goal_page(session_id, &page_id)?,
    };
    let snapshot = crate::side_panel::write_markdown_page(
        session_id,
        &page_id,
        Some(&page_title),
        &render_goal_detail(goal),
        focus,
    )?;
    attach_goal_to_session(session_id, goal, working_dir)?;
    Ok(snapshot)
}

pub fn goal_page_id(id: &str) -> String {
    format!("goal.{}", sanitize_id(id))
}

pub fn header_badge(
    working_dir: Option<&Path>,
    snapshot: &crate::side_panel::SidePanelSnapshot,
) -> Option<String> {
    if let Some(page) = snapshot.focused_page() {
        if page.id.starts_with("goal.") {
            return Some(format!("🎯 {}*", truncate_title(&page.title, 28)));
        }
    }

    let goals = list_relevant_goals(working_dir).ok()?;
    let active: Vec<_> = goals
        .into_iter()
        .filter(|goal| {
            matches!(
                goal.status,
                GoalStatus::Active | GoalStatus::Blocked | GoalStatus::Draft
            )
        })
        .collect();
    match active.as_slice() {
        [] => None,
        [goal] => Some(format!("🎯 {}", truncate_title(&goal.title, 28))),
        many => Some(format!("🎯 {} active", many.len())),
    }
}

pub fn render_goals_overview(goals: &[Goal]) -> String {
    let mut out = String::from("# Goals\n\n");
    if goals.is_empty() {
        out.push_str(
            "No goals yet. Use the `goal` tool or `/goals show <id>` after creating one.\n",
        );
        return out;
    }

    for goal in goals {
        out.push_str(&format!(
            "## {}\n- Status: {}\n- Scope: {}\n",
            goal.title,
            goal.status.as_str(),
            goal.scope.as_str()
        ));
        if let Some(progress) = goal.progress_percent {
            out.push_str(&format!("- Progress: {}%\n", progress));
        }
        if let Some(milestone) = goal.current_milestone() {
            out.push_str(&format!("- Current milestone: {}\n", milestone.title));
        }
        if let Some(next_step) = goal.next_steps.first() {
            out.push_str(&format!("- Next step: {}\n", next_step));
        }
        out.push_str(&format!("- Id: `{}`\n\n", goal.id));
    }
    out
}

pub fn render_goal_detail(goal: &Goal) -> String {
    let mut out = format!(
        "# Goal: {}\n\n**Status:** {}  \n**Scope:** {}  \n**Updated:** {}  \n",
        goal.title,
        goal.status.as_str(),
        goal.scope.as_str(),
        goal.updated_at.format("%Y-%m-%d %H:%M")
    );
    if let Some(progress) = goal.progress_percent {
        out.push_str(&format!("**Progress:** {}%  \n", progress));
    }
    out.push('\n');

    if !goal.description.trim().is_empty() {
        out.push_str("## Description\n");
        out.push_str(goal.description.trim());
        out.push_str("\n\n");
    }
    if !goal.why.trim().is_empty() {
        out.push_str("## Why\n");
        out.push_str(goal.why.trim());
        out.push_str("\n\n");
    }
    if !goal.success_criteria.is_empty() {
        out.push_str("## Success criteria\n");
        for item in &goal.success_criteria {
            out.push_str(&format!("- {}\n", item));
        }
        out.push('\n');
    }
    if let Some(milestone) = goal.current_milestone() {
        out.push_str(&format!("## Current milestone\n### {}\n", milestone.title));
        if milestone.steps.is_empty() {
            out.push_str(&format!("- Status: {}\n\n", milestone.status));
        } else {
            for step in &milestone.steps {
                let checked = if step.status == "completed" { "x" } else { " " };
                out.push_str(&format!("- [{}] {}\n", checked, step.content));
            }
            out.push('\n');
        }
    }
    if !goal.milestones.is_empty() {
        out.push_str("## Milestones\n");
        for milestone in &goal.milestones {
            let marker = if goal.current_milestone_id.as_deref() == Some(milestone.id.as_str()) {
                "→"
            } else {
                "-"
            };
            out.push_str(&format!(
                "{} {} ({})\n",
                marker, milestone.title, milestone.status
            ));
        }
        out.push('\n');
    }
    if !goal.next_steps.is_empty() {
        out.push_str("## Next steps\n");
        for (idx, step) in goal.next_steps.iter().enumerate() {
            out.push_str(&format!("{}. {}\n", idx + 1, step));
        }
        out.push('\n');
    }
    if !goal.blockers.is_empty() {
        out.push_str("## Blockers\n");
        for blocker in &goal.blockers {
            out.push_str(&format!("- {}\n", blocker));
        }
        out.push('\n');
    }
    if !goal.updates.is_empty() {
        out.push_str("## Recent updates\n");
        for update in goal.updates.iter().rev().take(8) {
            out.push_str(&format!(
                "- {}: {}\n",
                update.at.format("%Y-%m-%d"),
                update.summary
            ));
        }
    }
    out
}

fn should_focus_goal_page(session_id: &str, page_id: &str) -> Result<bool> {
    let snapshot = crate::side_panel::snapshot_for_session(session_id)?;
    let has_goal_page = snapshot
        .pages
        .iter()
        .any(|page| page.id == "goals" || page.id.starts_with("goal."));
    let focused = snapshot.focused_page_id.as_deref();
    Ok(!has_goal_page || focused == Some(page_id) || focused == Some("goals"))
}

fn save_goal(goal: &Goal, working_dir: Option<&Path>) -> Result<()> {
    let path = goal_file(goal, working_dir)?;
    crate::storage::write_json_fast(&path, goal)
}

fn goal_file(goal: &Goal, working_dir: Option<&Path>) -> Result<PathBuf> {
    let dir = match goal.scope {
        GoalScope::Global => global_goals_dir()?,
        GoalScope::Project => project_goals_dir(working_dir)?
            .ok_or_else(|| anyhow::anyhow!("working_dir required for project goal"))?,
    };
    Ok(goal_file_in_dir(&dir, &goal.id))
}

fn goal_file_in_dir(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{}.json", sanitize_id(id)))
}

fn global_goals_dir() -> Result<PathBuf> {
    Ok(crate::storage::jcode_dir()?.join("goals").join("global"))
}

fn project_goals_dir(working_dir: Option<&Path>) -> Result<Option<PathBuf>> {
    let Some(dir) = working_dir else {
        return Ok(None);
    };
    Ok(Some(
        crate::storage::jcode_dir()?
            .join("goals")
            .join("projects")
            .join(project_hash(dir)),
    ))
}

fn load_goals_in_dir(dir: &Path) -> Result<Vec<Goal>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut goals = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let goal: Goal = crate::storage::read_json(&path)
            .with_context(|| format!("failed to read goal {}", path.display()))?;
        goals.push(goal);
    }
    sort_goals(&mut goals);
    Ok(goals)
}

fn sort_goals(goals: &mut [Goal]) {
    goals.sort_by(|a, b| {
        a.status
            .sort_rank()
            .cmp(&b.status.sort_rank())
            .then_with(|| b.updated_at.cmp(&a.updated_at))
            .then_with(|| a.title.cmp(&b.title))
    });
}

fn project_hash(path: &Path) -> String {
    use std::collections::hash_map::DefaultHasher;
    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn session_attachment_path(session_id: &str) -> Result<PathBuf> {
    Ok(crate::storage::jcode_dir()?
        .join("goals")
        .join("sessions")
        .join(format!("{}.json", session_id)))
}

fn next_available_goal_id(
    base: &str,
    scope: GoalScope,
    working_dir: Option<&Path>,
) -> Result<String> {
    let mut candidate = sanitize_id(base);
    let mut idx = 2;
    while load_goal(&candidate, Some(scope), working_dir)?.is_some() {
        candidate = format!("{}-{}", sanitize_id(base), idx);
        idx += 1;
    }
    Ok(candidate)
}

fn sanitize_id(id: &str) -> String {
    let slug = slugify(id);
    if slug.is_empty() {
        "goal".to_string()
    } else {
        slug
    }
}

fn slugify(input: &str) -> String {
    let mut slug = String::new();
    let mut prev_dash = false;
    for ch in input.chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_alphanumeric() {
            slug.push(lower);
            prev_dash = false;
        } else if !prev_dash {
            slug.push('-');
            prev_dash = true;
        }
    }
    slug.trim_matches('-').to_string()
}

fn trim_vec(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect()
}

fn truncate_title(title: &str, max_chars: usize) -> String {
    let raw = title.trim_start_matches("Goal: ").trim();
    let char_count = raw.chars().count();
    if char_count <= max_chars {
        raw.to_string()
    } else if max_chars <= 1 {
        "…".to_string()
    } else {
        let clipped: String = raw.chars().take(max_chars - 1).collect();
        format!("{}…", clipped)
    }
}

fn sync_goal_memory(goal: &Goal, working_dir: Option<&Path>) -> Result<String> {
    use crate::memory::{MemoryCategory, MemoryEntry, MemoryManager, TrustLevel};

    let manager = match goal.scope {
        GoalScope::Project => {
            MemoryManager::new().with_project_dir(working_dir.ok_or_else(|| {
                anyhow::anyhow!("working_dir required for project goal memory sync")
            })?)
        }
        GoalScope::Global => MemoryManager::new(),
    };

    let mut entry = MemoryEntry::new(
        MemoryCategory::Custom("goal".to_string()),
        goal_memory_content(goal),
    )
    .with_source(format!("goal:{}", goal.id))
    .with_trust(TrustLevel::High)
    .with_tags(goal_memory_tags(goal));
    entry.id = goal_memory_id(goal);
    entry.updated_at = goal.updated_at;
    entry.created_at = goal.created_at;

    match goal.scope {
        GoalScope::Project => manager.upsert_project_memory(entry),
        GoalScope::Global => manager.upsert_global_memory(entry),
    }
}

fn goal_memory_id(goal: &Goal) -> String {
    format!("goal:{}", goal.id)
}

fn goal_memory_tags(goal: &Goal) -> Vec<String> {
    let mut tags = vec![
        "goal".to_string(),
        format!("goal:{}", goal.id),
        format!("goal_status:{}", goal.status.as_str()),
        format!("goal_scope:{}", goal.scope.as_str()),
    ];
    if let Some(current) = goal.current_milestone_id.as_deref() {
        tags.push(format!("goal_milestone:{}", current));
    }
    if !goal.title.trim().is_empty() {
        tags.extend(
            goal.title
                .split(|ch: char| !ch.is_ascii_alphanumeric())
                .map(|part| part.trim().to_ascii_lowercase())
                .filter(|part| part.len() >= 4)
                .take(4)
                .map(|part| format!("goal_term:{}", part)),
        );
    }
    tags.sort();
    tags.dedup();
    tags
}

fn goal_memory_content(goal: &Goal) -> String {
    let mut out = format!(
        "Goal: {}\nStatus: {}\nScope: {}",
        goal.title,
        goal.status.as_str(),
        goal.scope.as_str()
    );
    if let Some(progress) = goal.progress_percent {
        out.push_str(&format!("\nProgress: {}%", progress));
    }
    if let Some(milestone) = goal.current_milestone() {
        out.push_str(&format!("\nCurrent milestone: {}", milestone.title));
    }
    if !goal.description.trim().is_empty() {
        out.push_str(&format!("\nDescription: {}", goal.description.trim()));
    }
    if !goal.why.trim().is_empty() {
        out.push_str(&format!("\nWhy: {}", goal.why.trim()));
    }
    if !goal.next_steps.is_empty() {
        out.push_str("\nNext steps:");
        for step in goal.next_steps.iter().take(3) {
            out.push_str(&format!("\n- {}", step));
        }
    }
    if !goal.blockers.is_empty() {
        out.push_str("\nBlockers:");
        for blocker in goal.blockers.iter().take(3) {
            out.push_str(&format!("\n- {}", blocker));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_and_resume_goal_persists_project_goal() {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::tempdir().expect("tempdir");
        let project = temp.path().join("repo");
        std::fs::create_dir_all(&project).expect("project dir");
        let prev_home = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", temp.path());

        let goal = create_goal(
            GoalCreateInput {
                title: "Ship mobile MVP".to_string(),
                scope: GoalScope::Project,
                next_steps: vec!["finish reconnect flow".to_string()],
                progress_percent: Some(40),
                ..GoalCreateInput::default()
            },
            Some(&project),
        )
        .expect("create goal");
        assert_eq!(goal.id, "ship-mobile-mvp");

        let loaded = load_goal(&goal.id, Some(GoalScope::Project), Some(&project))
            .expect("load")
            .expect("goal exists");
        assert_eq!(loaded.title, "Ship mobile MVP");

        let manager = crate::memory::MemoryManager::new().with_project_dir(&project);
        let graph = manager.load_project_graph().expect("load graph");
        let goal_mem = graph
            .get_memory(&format!("goal:{}", goal.id))
            .expect("goal memory mirror");
        assert!(goal_mem.tags.iter().any(|tag| tag == "goal"));
        assert!(goal_mem.content.contains("Ship mobile MVP"));

        let session_id = "ses_goal_test";
        attach_goal_to_session(session_id, &goal, Some(&project)).expect("attach");
        let resumed = resume_goal(session_id, Some(&project))
            .expect("resume")
            .expect("goal resumed");
        assert_eq!(resumed.id, goal.id);

        if let Some(prev_home) = prev_home {
            crate::env::set_var("JCODE_HOME", prev_home);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }

    #[test]
    fn write_goal_page_auto_focuses_first_goal_only() {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::tempdir().expect("tempdir");
        let project = temp.path().join("repo");
        std::fs::create_dir_all(&project).expect("project dir");
        let prev_home = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", temp.path());

        let session_id = "ses_goal_panel";
        let goal = create_goal(
            GoalCreateInput {
                title: "Ship mobile MVP".to_string(),
                scope: GoalScope::Project,
                ..GoalCreateInput::default()
            },
            Some(&project),
        )
        .expect("create goal");

        let first = write_goal_page(session_id, Some(&project), &goal, GoalDisplayMode::Auto)
            .expect("first write");
        assert_eq!(
            first.focused_page_id.as_deref(),
            Some("goal.ship-mobile-mvp")
        );

        crate::side_panel::write_markdown_page(session_id, "notes", Some("Notes"), "# Notes", true)
            .expect("notes");
        let second = write_goal_page(session_id, Some(&project), &goal, GoalDisplayMode::Auto)
            .expect("second write");
        assert_eq!(second.focused_page_id.as_deref(), Some("notes"));

        if let Some(prev_home) = prev_home {
            crate::env::set_var("JCODE_HOME", prev_home);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }
}
