use super::{Registry, Tool, ToolContext, ToolOutput};
use crate::agent::Agent;
use crate::bus::{Bus, BusEvent, ToolSummary, ToolSummaryState};
use crate::logging;
use crate::protocol::HistoryMessage;
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
    #[serde(default)]
    output_mode: SubagentOutputMode,
    #[serde(rename = "command", default)]
    _command: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum SubagentOutputMode {
    /// Return only the subagent's final answer plus metadata. This preserves the
    /// historical low-token default for ordinary delegation.
    #[default]
    Answer,
    /// Return the final answer plus a human-readable transcript similar to what
    /// a user would inspect: roles, text, tool calls, and tool results.
    Compact,
    /// Return the final answer plus the persisted raw child session messages as
    /// pretty JSON for debugging/auditing.
    FullTranscript,
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
                "intent": super::intent_schema_property(),
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
                "output_mode": {
                    "type": "string",
                    "enum": ["answer", "compact", "full_transcript"],
                    "description": "Return mode. 'answer' returns the final answer only, 'compact' adds a user-visible transcript, and 'full_transcript' adds raw persisted messages. Defaults to 'answer'."
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
        crate::config::config()
            .tools
            .apply_to_allowed_set(&mut allowed);

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
        let history = if params.output_mode == SubagentOutputMode::Compact {
            Some(agent.get_history())
        } else {
            None
        };
        let full_transcript = if params.output_mode == SubagentOutputMode::FullTranscript {
            let session = Session::load(&sub_session_id)?;
            Some(serde_json::to_string_pretty(&session.messages)?)
        } else {
            None
        };

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

        let output = format_subagent_output(
            &final_text,
            &sub_session_id,
            params.output_mode,
            history.as_deref(),
            full_transcript.as_deref(),
        );

        Ok(ToolOutput::new(output)
            .with_title(subagent_display_title(&params, &resolved_model))
            .with_metadata(json!({
                "summary": summary,
                "sessionId": sub_session_id,
                "model": resolved_model,
                "outputMode": params.output_mode.as_str(),
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

impl SubagentOutputMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Answer => "answer",
            Self::Compact => "compact",
            Self::FullTranscript => "full_transcript",
        }
    }
}

fn format_subagent_output(
    final_text: &str,
    sub_session_id: &str,
    output_mode: SubagentOutputMode,
    history: Option<&[HistoryMessage]>,
    full_transcript: Option<&str>,
) -> String {
    let mut output = final_text.to_string();
    if !output.ends_with('\n') {
        output.push('\n');
    }

    match output_mode {
        SubagentOutputMode::Answer => {}
        SubagentOutputMode::Compact => {
            output.push_str("\n## Subagent transcript (compact)\n\n");
            output.push_str(&format_compact_subagent_history(history.unwrap_or(&[])));
        }
        SubagentOutputMode::FullTranscript => {
            output.push_str("\n## Subagent transcript (full)\n\n```json\n");
            output.push_str(full_transcript.unwrap_or("[]"));
            output.push_str("\n```\n");
        }
    }

    output.push('\n');
    output.push_str("<subagent_metadata>\n");
    output.push_str(&format!("session_id: {}\n", sub_session_id));
    output.push_str(&format!("output_mode: {}\n", output_mode.as_str()));
    output.push_str("</subagent_metadata>");
    output
}

fn format_compact_subagent_history(messages: &[HistoryMessage]) -> String {
    if messages.is_empty() {
        return "(empty transcript)\n".to_string();
    }

    let mut output = String::new();
    for (index, message) in messages.iter().enumerate() {
        output.push_str(&format!("### {}. {}\n\n", index + 1, message.role));
        if !message.content.trim().is_empty() {
            output.push_str(message.content.trim());
            output.push_str("\n\n");
        }
        if let Some(tool_calls) = &message.tool_calls
            && !tool_calls.is_empty()
        {
            output.push_str("Tool calls:\n");
            for call in tool_calls {
                output.push_str(&format!("- `{}`\n", call));
            }
            output.push('\n');
        }
        if let Some(tool_data) = &message.tool_data {
            output.push_str("Tool result:\n");
            output.push_str("```json\n");
            match serde_json::to_string_pretty(tool_data) {
                Ok(json) => output.push_str(&json),
                Err(_) => output.push_str("<unserializable tool data>"),
            }
            output.push_str("\n```\n\n");
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::{
        SubagentInput, SubagentOutputMode, format_compact_subagent_history, format_subagent_output,
        subagent_display_title,
    };
    use crate::protocol::HistoryMessage;

    #[test]
    fn subagent_display_title_includes_type_and_model() {
        let params = SubagentInput {
            description: "Verify subagent model".to_string(),
            prompt: "prompt".to_string(),
            subagent_type: "general".to_string(),
            model: None,
            session_id: None,
            output_mode: SubagentOutputMode::Answer,
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
        let configured_or_provider = crate::config::config()
            .agents
            .swarm_model
            .as_deref()
            .unwrap_or("provider");
        assert_eq!(
            super::SubagentTool::resolve_model(None, None, None, "provider"),
            configured_or_provider
        );
    }

    #[test]
    fn format_subagent_output_preserves_answer_without_generic_next_step_footer() {
        let output = format_subagent_output(
            "answer",
            "session_test",
            SubagentOutputMode::Answer,
            None,
            None,
        );

        assert!(output.starts_with("answer\n\n<subagent_metadata>\n"));
        assert!(output.contains("session_id: session_test\n"));
        assert!(output.contains("output_mode: answer\n"));
        assert!(!output.contains("Next step: integrate this result"));
    }

    #[test]
    fn compact_output_includes_human_readable_history() {
        let history = vec![HistoryMessage {
            role: "assistant".to_string(),
            content: "I will inspect it.".to_string(),
            tool_calls: Some(vec!["read".to_string()]),
            tool_data: None,
        }];
        let output = format_subagent_output(
            "final answer",
            "session_test",
            SubagentOutputMode::Compact,
            Some(&history),
            None,
        );

        assert!(output.contains("## Subagent transcript (compact)"));
        assert!(output.contains("### 1. assistant"));
        assert!(output.contains("I will inspect it."));
        assert!(output.contains("- `read`"));
        assert!(output.contains("output_mode: compact\n"));
    }

    #[test]
    fn full_transcript_output_includes_raw_json_section() {
        let output = format_subagent_output(
            "final answer",
            "session_test",
            SubagentOutputMode::FullTranscript,
            None,
            Some("[{\"role\":\"user\"}]"),
        );

        assert!(output.contains("## Subagent transcript (full)"));
        assert!(output.contains("```json\n[{\"role\":\"user\"}]\n```"));
        assert!(output.contains("output_mode: full_transcript\n"));
    }

    #[test]
    fn compact_history_formats_empty_transcript() {
        assert_eq!(format_compact_subagent_history(&[]), "(empty transcript)\n");
    }
}
