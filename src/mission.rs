use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const MISSION_CONTINUATION_TEMPLATE: &str = include_str!("prompt/mission_continuation.md");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MissionStatus {
    Active,
    Paused,
    Blocked,
    NeedsDecision,
    BudgetLimited,
    Complete,
    Abandoned,
}

impl MissionStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Blocked => "blocked",
            Self::NeedsDecision => "needs_decision",
            Self::BudgetLimited => "budget_limited",
            Self::Complete => "complete",
            Self::Abandoned => "abandoned",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MissionCheckpoint {
    pub at: DateTime<Utc>,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Mission {
    pub session_id: String,
    pub objective: String,
    pub long_horizon_intent: String,
    pub status: MissionStatus,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub semantic_expansion: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub success_criteria: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub validation_plan: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checkpoints: Vec<MissionCheckpoint>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub fn load(session_id: &str) -> Result<Option<Mission>> {
    let path = mission_path(session_id)?;
    if !path.exists() {
        return Ok(None);
    }
    crate::storage::read_json(&path)
}

pub fn set(session_id: &str, objective: &str) -> Result<Mission> {
    let objective = objective.trim();
    if objective.is_empty() {
        anyhow::bail!("mission objective cannot be empty");
    }
    let now = Utc::now();
    let mut mission = load(session_id)?.unwrap_or_else(|| Mission {
        session_id: session_id.to_string(),
        objective: String::new(),
        long_horizon_intent: String::new(),
        status: MissionStatus::Active,
        semantic_expansion: Vec::new(),
        success_criteria: Vec::new(),
        validation_plan: Vec::new(),
        checkpoints: Vec::new(),
        created_at: now,
        updated_at: now,
    });
    mission.objective = objective.to_string();
    mission.long_horizon_intent = default_long_horizon_intent(objective);
    mission.status = MissionStatus::Active;
    mission.updated_at = now;
    save(&mission)?;
    Ok(mission)
}

pub fn update_status(session_id: &str, status: MissionStatus) -> Result<Option<Mission>> {
    let Some(mut mission) = load(session_id)? else {
        return Ok(None);
    };
    mission.status = status;
    mission.updated_at = Utc::now();
    save(&mission)?;
    Ok(Some(mission))
}

pub fn checkpoint(session_id: &str, summary: &str) -> Result<Option<Mission>> {
    let summary = summary.trim();
    if summary.is_empty() {
        anyhow::bail!("checkpoint summary cannot be empty");
    }
    let Some(mut mission) = load(session_id)? else {
        return Ok(None);
    };
    mission.checkpoints.push(MissionCheckpoint {
        at: Utc::now(),
        summary: summary.to_string(),
    });
    mission.updated_at = Utc::now();
    save(&mission)?;
    Ok(Some(mission))
}

pub fn clear(session_id: &str) -> Result<bool> {
    let path = mission_path(session_id)?;
    if path.exists() {
        std::fs::remove_file(path)?;
        Ok(true)
    } else {
        Ok(false)
    }
}

pub fn render_status(mission: &Mission) -> String {
    let mut out = format!(
        "Mission **{}**\n\nStatus: **{}**\n\nLong-horizon intent: {}",
        mission.objective,
        mission.status.as_str(),
        mission.long_horizon_intent
    );
    if let Some(last) = mission.checkpoints.last() {
        out.push_str(&format!("\n\nLast checkpoint: {}", last.summary));
    }
    out.push_str("\n\nMission loop: keep updating todos, expand adjacent work, validate progress, and continue until complete, blocked, paused, or a decision is needed.");
    out
}

pub fn active_system_reminder(session_id: &str) -> Result<Option<String>> {
    let Some(mission) = load(session_id)? else {
        return Ok(None);
    };
    if !matches!(mission.status, MissionStatus::Active) {
        return Ok(None);
    }
    Ok(Some(render_mission_continuation_prompt(&mission)))
}

pub fn render_mission_continuation_prompt(mission: &Mission) -> String {
    MISSION_CONTINUATION_TEMPLATE
        .replace("{{ objective }}", &escape_xml_text(&mission.objective))
        .replace(
            "{{ long_horizon_intent }}",
            &escape_xml_text(&mission.long_horizon_intent),
        )
}

fn escape_xml_text(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn save(mission: &Mission) -> Result<()> {
    crate::storage::write_json_fast(&mission_path(&mission.session_id)?, mission)
}

fn mission_path(session_id: &str) -> Result<PathBuf> {
    Ok(crate::storage::jcode_dir()?
        .join("missions")
        .join(format!("{}.json", sanitize_session_id(session_id))))
}

fn sanitize_session_id(session_id: &str) -> String {
    session_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn default_long_horizon_intent(objective: &str) -> String {
    format!(
        "Interpret `{}` broadly: pursue the literal objective, continuously refresh the todo frontier, include semantically adjacent work that improves the outcome, and preserve long-term quality.",
        objective
    )
}
