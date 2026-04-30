use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

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

    pub fn sort_rank(self) -> u8 {
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
            id: sanitize_goal_id(trimmed),
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

pub fn sanitize_goal_id(id: &str) -> String {
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

fn default_pending_status() -> String {
    "pending".to_string()
}
