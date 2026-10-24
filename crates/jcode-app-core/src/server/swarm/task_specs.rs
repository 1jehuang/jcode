use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(super) struct SwarmTaskSpec {
    pub(super) description: String,
    pub(super) prompt: String,
    #[serde(default)]
    pub(super) subagent_type: Option<String>,
}

pub(super) fn parse_swarm_tasks(text: &str) -> Vec<SwarmTaskSpec> {
    if let Ok(tasks) = serde_json::from_str::<Vec<SwarmTaskSpec>>(text) {
        return tasks;
    }

    if let (Some(start), Some(end)) = (text.find('['), text.rfind(']'))
        && start < end
        && let Ok(tasks) = serde_json::from_str::<Vec<SwarmTaskSpec>>(&text[start..=end])
    {
        return tasks;
    }

    Vec::new()
}
