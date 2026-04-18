use super::{Registry, Tool, ToolContext, ToolOutput};
use crate::agent::Agent;
use crate::bus::{Bus, BusEvent, ToolSummary, ToolSummaryState};
use crate::logging;
use crate::provider::Provider;
use crate::session::Session;
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

pub struct SubagentTool {
    provider: Arc<dyn Provider>,
    registry: Registry,
}

impl SubagentTool {
    pub fn new(provider: Arc<dyn Provider>, registry: Registry) -> Self {
        Self { provider, registry }
    }

    fn preferred_parent_subagent_model(parent_session_id: &str) -> Option<String> {
        Session::load(parent_session_id)
            .ok()
            .and_then(|session| session.subagent_model)
    }

    fn resolve_model(
        requested_model: Option<&str>,
        existing_session_model: Option<&str>,
        parent_subagent_model: Option<&str>,
        provider_model: &str,
    ) -> String {
        requested_model
            .or(existing_session_model)
            .or(parent_subagent_model)
            .or(crate::config::config().agents.swarm_model.as_deref())
            .unwrap_or(provider_model)
            .to_string()
    }
}

#[derive(Deserialize)]
struct SubagentInput {
    description: String,
    prompt: String,
    subagent_type: String,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(rename = "command", default)]
    _command: Option<String>,
}

#[async_trait]
impl Tool for SubagentTool {
    fn name(&self) -> &str {
        "subagent"
    }

    fn description(&self) -> &str {
        "Run a subagent."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["description", "prompt", "subagent_type"],
            "properties": {
                "description": {
                    "type": "string",
                    "description": "Task description."
                },
                "prompt": {
                    "type": "string",
                    "description": "Task prompt."
                },
                "subagent_type": {
                    "type": "string",
                    "description": "Subagent type."
                },
                "model": {
                    "type": "string",
                    "description": "Model override."
                },
                "session_id": {
                    "type": "string",
                    "description": "Existing session ID."
                },
                "command": {
                    "type": "string",
                    "description": "Source command."
                }
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let params: SubagentInput = serde_json::from_value(input)?;

        let mut session = if let Some(session_id) = &params.session_id {
            Session::load(session_id).unwrap_or_else(|err| {
                logging::warn(&format!(
                    "[tool:subagent] failed to load existing session {}; creating a new subagent session instead: {}",
                    session_id, err
                ));
                Session::create(Some(ctx.session_id.clone()), Some(subagent_title(&params)))
            })
        } else {
            Session::create(Some(ctx.session_id.clone()), Some(subagent_title(&params)))
        };
        let parent_subagent_model = Self::preferred_parent_subagent_model(&ctx.session_id);
        let provider_model = self.provider.model();
        let resolved_model = Self::resolve_model(
            params.model.as_deref(),
            session.model.as_deref(),
            parent_subagent_model.as_deref(),
            &provider_model,
        );
        session.model = Some(resolved_model.clone());

        if let Some(ref working_dir) = ctx.working_dir {
            session.working_dir = Some(working_dir.display().to_string());
        }

        session.save()?;

        let mut allowed: HashSet<String> = self.registry.tool_names().await.into_iter().collect();
        for blocked in ["subagent", "task", "todo", "todowrite", "todoread"] {
            allowed.remove(blocked);
        }

        let summary_map: Arc<Mutex<HashMap<String, ToolSummary>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let summary_map_handle = summary_map.clone();
        let session_id = session.id.clone();

        let mut receiver = Bus::global().subscribe();
        let listener = tokio::spawn(async move {
            loop {
                match receiver.recv().await {
                    Ok(BusEvent::ToolUpdated(event)) => {
                        if event.session_id != session_id {
                            continue;
                        }
                        let mut summary = summary_map_handle
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        summary.insert(
                            event.tool_call_id.clone(),
                            ToolSummary {
                                id: event.tool_call_id.clone(),
                                tool: event.tool_name.clone(),
                                state: ToolSummaryState {
                                    status: event.status.as_str().to_string(),
                                    title: if event.status.as_str() == "completed" {
                                        event.title.clone()
                                    } else {
                                        None
                                    },
                                },
                            },
                        );
                    }
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Closed) => break,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                }
            }
        });

        logging::info(&format!(
            "Subagent starting: {} (type: {})",
            params.description, params.subagent_type
        ));

        // Run subagent on an isolated provider fork so model/session changes do not
        // mutate the coordinator's provider instance.
        let mut agent = Agent::new_with_session(
            self.provider.fork(),
            self.registry.clone(),
            session,
            Some(allowed),
        );

        let start = std::time::Instant::now();
        let final_text = agent.run_once_capture(&params.prompt).await.map_err(|err| {
            logging::warn(&format!(
                "[tool:subagent] subagent failed description={} type={} session_id={} model={} error={}",
                params.description,
                params.subagent_type,
                agent.session_id(),
                resolved_model,
                err
            ));
            err
        })?;
        let sub_session_id = agent.session_id().to_string();

        logging::info(&format!(
            "Subagent completed: {} in {:.1}s",
            params.description,
            start.elapsed().as_secs_f64()
        ));

        listener.abort();

        let mut summary: Vec<ToolSummary> = summary_map
            .lock()
            .map_err(|_| anyhow::anyhow!("tool summary lock poisoned"))?
            .values()
            .cloned()
            .collect();
        summary.sort_by(|a, b| a.id.cmp(&b.id));

        let mut output = final_text;
        if !output.ends_with('\n') {
            output.push('\n');
        }
        output.push('\n');
        output.push_str("Next step: integrate this result into the main task and continue.\n");
        output.push('\n');
        output.push_str("<subagent_metadata>\n");
        output.push_str(&format!("session_id: {}\n", sub_session_id));
        output.push_str("</subagent_metadata>");

        Ok(ToolOutput::new(output)
            .with_title(subagent_display_title(&params, &resolved_model))
            .with_metadata(json!({
                "summary": summary,
                "sessionId": sub_session_id,
                "model": resolved_model,
            })))
    }
}

fn subagent_title(params: &SubagentInput) -> String {
    format!(
        "{} (@{} subagent)",
        params.description, params.subagent_type
    )
}

fn subagent_display_title(params: &SubagentInput, model: &str) -> String {
    format!(
        "{} ({} · {})",
        params.description, params.subagent_type, model
    )
}

#[cfg(test)]
mod tests {
    use super::{SubagentInput, subagent_display_title};

    #[test]
    fn subagent_display_title_includes_type_and_model() {
        let params = SubagentInput {
            description: "Verify subagent model".to_string(),
            prompt: "prompt".to_string(),
            subagent_type: "general".to_string(),
            model: None,
            session_id: None,
            _command: None,
        };

        assert_eq!(
            subagent_display_title(&params, "gpt-5.4"),
            "Verify subagent model (general · gpt-5.4)"
        );
    }

    #[test]
    fn resolve_model_prefers_explicit_then_existing_then_parent_then_provider() {
        assert_eq!(
            super::SubagentTool::resolve_model(
                Some("explicit"),
                Some("existing"),
                Some("parent"),
                "provider"
            ),
            "explicit"
        );
        assert_eq!(
            super::SubagentTool::resolve_model(None, Some("existing"), Some("parent"), "provider"),
            "existing"
        );
        assert_eq!(
            super::SubagentTool::resolve_model(None, None, Some("parent"), "provider"),
            "parent"
        );
        assert_eq!(
            super::SubagentTool::resolve_model(None, None, None, "provider"),
            "provider"
        );
    }
}
