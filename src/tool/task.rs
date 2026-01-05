use super::{Registry, Tool, ToolContext, ToolOutput};
use crate::agent::Agent;
use crate::bus::{Bus, BusEvent, ToolSummary, ToolSummaryState};
use crate::provider::Provider;
use crate::session::Session;
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

pub struct TaskTool {
    provider: Arc<dyn Provider>,
    registry: Registry,
}

impl TaskTool {
    pub fn new(provider: Arc<dyn Provider>, registry: Registry) -> Self {
        Self { provider, registry }
    }
}

#[derive(Deserialize)]
struct TaskInput {
    description: String,
    prompt: String,
    subagent_type: String,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    command: Option<String>,
}

#[async_trait]
impl Tool for TaskTool {
    fn name(&self) -> &str {
        "task"
    }

    fn description(&self) -> &str {
        "Run a sub-task using a dedicated subagent session. Returns the subagent output and a task session id."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["description", "prompt", "subagent_type"],
            "properties": {
                "description": {
                    "type": "string",
                    "description": "A short (3-5 words) description of the task"
                },
                "prompt": {
                    "type": "string",
                    "description": "The task for the agent to perform"
                },
                "subagent_type": {
                    "type": "string",
                    "description": "The type of specialized agent to use for this task"
                },
                "session_id": {
                    "type": "string",
                    "description": "Existing Task session to continue"
                },
                "command": {
                    "type": "string",
                    "description": "The command that triggered this task"
                }
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let params: TaskInput = serde_json::from_value(input)?;

        let mut session = if let Some(session_id) = &params.session_id {
            Session::load(session_id).unwrap_or_else(|_| {
                Session::create(Some(ctx.session_id.clone()), Some(task_title(&params)))
            })
        } else {
            Session::create(Some(ctx.session_id.clone()), Some(task_title(&params)))
        };

        session.save()?;

        let mut allowed: HashSet<String> = self
            .registry
            .tool_names()
            .await
            .into_iter()
            .collect();
        for blocked in ["task", "todowrite", "todoread"] {
            allowed.remove(blocked);
        }

        let summary_map: Arc<Mutex<HashMap<String, ToolSummary>>> = Arc::new(Mutex::new(HashMap::new()));
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
                        let mut summary = summary_map_handle.lock().expect("tool summary lock");
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

        let mut agent = Agent::new_with_session(
            self.provider.clone(),
            self.registry.clone(),
            session,
            Some(allowed),
        );

        let final_text = agent.run_once_capture(&params.prompt).await?;
        let sub_session_id = agent.session_id().to_string();

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
        output.push_str("<task_metadata>\n");
        output.push_str(&format!("session_id: {}\n", sub_session_id));
        output.push_str("</task_metadata>");

        Ok(ToolOutput::new(output)
            .with_title(params.description)
            .with_metadata(json!({
                "summary": summary,
                "sessionId": sub_session_id,
            })))
    }
}

fn task_title(params: &TaskInput) -> String {
    format!("{} (@{} subagent)", params.description, params.subagent_type)
}
