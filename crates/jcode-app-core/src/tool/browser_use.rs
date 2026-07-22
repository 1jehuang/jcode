use super::{Tool, ToolContext, ToolOutput};
use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::Duration;

const BASE_URL: &str = "https://api.browser-use.com/api/v3";
const API_KEY_ENVS: [&str; 2] = ["BROWSER_USE_API_KEY", "JCODE_BROWSER_USE_API_KEY"];
const DEFAULT_POLL_SECS: u64 = 5;
const DEFAULT_WAIT_TIMEOUT_SECS: u64 = 600;

/// Browser Use Cloud integration: dispatch natural-language web tasks to a
/// hosted stealth browser agent and poll for results.
pub struct BrowserUseTool {
    client: reqwest::Client,
}

impl BrowserUseTool {
    pub fn new() -> Self {
        Self {
            client: crate::provider::shared_http_client(),
        }
    }

    fn api_key() -> Result<String> {
        for env in API_KEY_ENVS {
            if let Ok(key) = std::env::var(env) {
                let key = key.trim().to_string();
                if !key.is_empty() {
                    return Ok(key);
                }
            }
        }
        bail!(
            "Browser Use API key not found. Set BROWSER_USE_API_KEY (keys start with bu_). \
             Create one at https://cloud.browser-use.com/settings?tab=api-keys&new=1"
        )
    }

    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value> {
        let key = Self::api_key()?;
        let url = format!("{BASE_URL}{path}");
        let mut req = self
            .client
            .request(method, &url)
            .header("X-Browser-Use-API-Key", key)
            .timeout(Duration::from_secs(60));
        if let Some(body) = body {
            req = req.json(&body);
        }
        let resp = req.send().await.context("Browser Use API request failed")?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            bail!("Browser Use API error {status}: {text}");
        }
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&text).context("Browser Use API returned invalid JSON")
    }

    async fn get_session(&self, session_id: &str) -> Result<Value> {
        self.request(reqwest::Method::GET, &format!("/sessions/{session_id}"), None)
            .await
    }

    fn summarize_session(session: &Value) -> String {
        let id = session["id"].as_str().unwrap_or("?");
        let status = session["status"].as_str().unwrap_or("?");
        let mut out = format!("Session {id}\nStatus: {status}");
        if let Some(title) = session["title"].as_str() {
            out.push_str(&format!("\nTitle: {title}"));
        }
        if let Some(success) = session["isTaskSuccessful"].as_bool() {
            out.push_str(&format!("\nTask successful: {success}"));
        }
        if let Some(step) = session["lastStepSummary"].as_str() {
            out.push_str(&format!("\nLast step: {step}"));
        }
        if let Some(live) = session["liveUrl"].as_str() {
            out.push_str(&format!("\nLive view: {live}"));
        }
        if let Some(cost) = session["totalCostUsd"].as_str() {
            out.push_str(&format!("\nTotal cost: ${cost}"));
        }
        let output = &session["output"];
        if !output.is_null() {
            let rendered = match output.as_str() {
                Some(s) => s.to_string(),
                None => serde_json::to_string_pretty(output).unwrap_or_default(),
            };
            out.push_str(&format!("\n\nOutput:\n{rendered}"));
        }
        out
    }
}

#[derive(Deserialize)]
struct BrowserUseInput {
    action: String,
    #[serde(default)]
    task: Option<String>,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    keep_alive: Option<bool>,
    #[serde(default)]
    max_cost_usd: Option<f64>,
    #[serde(default)]
    output_schema: Option<Value>,
    #[serde(default)]
    wait: Option<bool>,
    #[serde(default)]
    wait_timeout_secs: Option<u64>,
    #[serde(default)]
    stop_strategy: Option<String>,
    #[serde(default)]
    limit: Option<u64>,
}

#[async_trait]
impl Tool for BrowserUseTool {
    fn name(&self) -> &str {
        "browser_use"
    }

    fn description(&self) -> &str {
        "Run web tasks with the Browser Use Cloud agent: a hosted stealth browser driven by \
         a state-of-the-art browser agent. Give it a natural-language task (e.g. research, \
         form filling, data extraction on real websites) and get the result back. Use for \
         web automation that the local browser tool cannot handle (CAPTCHAs, proxies, \
         long-running autonomous browsing). Requires BROWSER_USE_API_KEY."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["action"],
            "properties": {
                "intent": super::intent_schema_property(),
                "action": {
                    "type": "string",
                    "enum": ["run_task", "status", "messages", "stop", "list"],
                    "description": "run_task dispatches a task (new session, or existing one via session_id). status polls a session. messages lists a session's recent agent messages. stop stops a session or its task. list shows recent sessions."
                },
                "task": {
                    "type": "string",
                    "description": "Natural-language task for the agent. Required for run_task."
                },
                "session_id": {
                    "type": "string",
                    "description": "Existing session ID (for follow-up tasks, status, messages, stop)."
                },
                "model": {
                    "type": "string",
                    "description": "Agent model, e.g. bu-mini (fast/cheap), bu-max (balanced), bu-ultra (most capable). Defaults to the Browser Use default."
                },
                "keep_alive": {
                    "type": "boolean",
                    "description": "Keep the session idle after the task for follow-up tasks. Defaults to false."
                },
                "max_cost_usd": {
                    "type": "number",
                    "description": "Maximum session cost in USD. The task is stopped if reached."
                },
                "output_schema": {
                    "type": "object",
                    "description": "Optional JSON Schema the agent's final output must conform to."
                },
                "wait": {
                    "type": "boolean",
                    "description": "For run_task: block and poll until the task finishes. Defaults to true. Set false to return immediately with the session ID."
                },
                "wait_timeout_secs": {
                    "type": "integer",
                    "description": "Max seconds to wait when wait=true. Defaults to 600."
                },
                "stop_strategy": {
                    "type": "string",
                    "enum": ["task", "session"],
                    "description": "For stop: 'task' stops the running task but keeps the session idle; 'session' destroys it. Defaults to session."
                },
                "limit": {
                    "type": "integer",
                    "description": "Max items for list/messages."
                }
            }
        })
    }

    async fn execute(&self, input: Value, _ctx: ToolContext) -> Result<ToolOutput> {
        let params: BrowserUseInput = serde_json::from_value(input)?;
        match params.action.as_str() {
            "run_task" => {
                let task = params
                    .task
                    .clone()
                    .filter(|t| !t.trim().is_empty())
                    .ok_or_else(|| anyhow!("run_task requires a nonempty 'task'"))?;
                let mut body = json!({ "task": task });
                if let Some(model) = &params.model {
                    body["model"] = json!(model);
                }
                if let Some(session_id) = &params.session_id {
                    body["sessionId"] = json!(session_id);
                }
                if let Some(keep_alive) = params.keep_alive {
                    body["keepAlive"] = json!(keep_alive);
                }
                if let Some(max_cost) = params.max_cost_usd {
                    body["maxCostUsd"] = json!(max_cost);
                }
                if let Some(schema) = &params.output_schema {
                    body["outputSchema"] = schema.clone();
                }
                let session = self
                    .request(reqwest::Method::POST, "/sessions", Some(body))
                    .await?;
                let session_id = session["id"]
                    .as_str()
                    .ok_or_else(|| anyhow!("Browser Use API response missing session id"))?
                    .to_string();

                if !params.wait.unwrap_or(true) {
                    return Ok(ToolOutput::new(Self::summarize_session(&session)));
                }

                let timeout =
                    Duration::from_secs(params.wait_timeout_secs.unwrap_or(DEFAULT_WAIT_TIMEOUT_SECS));
                let started = std::time::Instant::now();
                let mut latest = session;
                loop {
                    let status = latest["status"].as_str().unwrap_or("");
                    match status {
                        "stopped" | "timed_out" | "error" => break,
                        "idle" => {
                            // Idle after a task means the task finished (keep_alive sessions).
                            if !latest["output"].is_null()
                                || latest["isTaskSuccessful"].as_bool().is_some()
                            {
                                break;
                            }
                        }
                        _ => {}
                    }
                    if started.elapsed() >= timeout {
                        let mut out = Self::summarize_session(&latest);
                        out.push_str(&format!(
                            "\n\nTimed out waiting after {}s. The task may still be running; \
                             poll with action=status session_id={session_id}.",
                            timeout.as_secs()
                        ));
                        return Ok(ToolOutput::new(out));
                    }
                    tokio::time::sleep(Duration::from_secs(DEFAULT_POLL_SECS)).await;
                    latest = self.get_session(&session_id).await?;
                }
                Ok(ToolOutput::new(Self::summarize_session(&latest)))
            }
            "status" => {
                let session_id = params
                    .session_id
                    .ok_or_else(|| anyhow!("status requires 'session_id'"))?;
                let session = self.get_session(&session_id).await?;
                Ok(ToolOutput::new(Self::summarize_session(&session)))
            }
            "messages" => {
                let session_id = params
                    .session_id
                    .ok_or_else(|| anyhow!("messages requires 'session_id'"))?;
                let limit = params.limit.unwrap_or(20).min(100);
                let resp = self
                    .request(
                        reqwest::Method::GET,
                        &format!("/sessions/{session_id}/messages?limit={limit}"),
                        None,
                    )
                    .await?;
                let mut out = format!("Messages for session {session_id}:\n");
                let empty = Vec::new();
                let messages = resp["messages"].as_array().unwrap_or(&empty);
                if messages.is_empty() {
                    out.push_str("(none)");
                }
                for msg in messages {
                    let role = msg["role"].as_str().unwrap_or("?");
                    let summary = msg["summary"].as_str().unwrap_or("");
                    let data = msg["data"].as_str().unwrap_or("");
                    let line = if summary.is_empty() { data } else { summary };
                    let mut line = line.trim().to_string();
                    if line.len() > 300 {
                        line.truncate(300);
                        line.push_str("...");
                    }
                    out.push_str(&format!("[{role}] {line}\n"));
                }
                Ok(ToolOutput::new(out))
            }
            "stop" => {
                let session_id = params
                    .session_id
                    .ok_or_else(|| anyhow!("stop requires 'session_id'"))?;
                let strategy = params.stop_strategy.unwrap_or_else(|| "session".to_string());
                let session = self
                    .request(
                        reqwest::Method::POST,
                        &format!("/sessions/{session_id}/stop"),
                        Some(json!({ "strategy": strategy })),
                    )
                    .await?;
                Ok(ToolOutput::new(Self::summarize_session(&session)))
            }
            "list" => {
                let limit = params.limit.unwrap_or(10).min(100);
                let resp = self
                    .request(
                        reqwest::Method::GET,
                        &format!("/sessions?page=1&page_size={limit}"),
                        None,
                    )
                    .await?;
                let empty = Vec::new();
                let sessions = resp["sessions"].as_array().unwrap_or(&empty);
                let total = resp["total"].as_u64().unwrap_or(sessions.len() as u64);
                let mut out = format!("Browser Use sessions ({total} total):\n");
                if sessions.is_empty() {
                    out.push_str("(none)");
                }
                for s in sessions {
                    let id = s["id"].as_str().unwrap_or("?");
                    let status = s["status"].as_str().unwrap_or("?");
                    let title = s["title"].as_str().unwrap_or("");
                    out.push_str(&format!("- {id} [{status}] {title}\n"));
                }
                Ok(ToolOutput::new(out))
            }
            other => bail!("Unknown browser_use action: {other}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_deserializes_minimal() {
        let input: BrowserUseInput =
            serde_json::from_value(json!({"action": "run_task", "task": "hi"})).unwrap();
        assert_eq!(input.action, "run_task");
        assert_eq!(input.task.as_deref(), Some("hi"));
        assert!(input.wait.is_none());
    }

    #[test]
    fn summarize_session_includes_output() {
        let session = json!({
            "id": "abc",
            "status": "stopped",
            "isTaskSuccessful": true,
            "output": "42",
            "totalCostUsd": "0.10"
        });
        let s = BrowserUseTool::summarize_session(&session);
        assert!(s.contains("Session abc"));
        assert!(s.contains("Task successful: true"));
        assert!(s.contains("42"));
    }
}
