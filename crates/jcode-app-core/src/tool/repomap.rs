//! Opt-in structural repo map tool (`#1230`). The model calls it to get a
//! token-budgeted skeleton of the codebase (ranked files plus symbol stubs)
//! instead of reading whole files blind. Read-only: walks, parses, ranks,
//! renders. Never writes except its own cache file. Only registered when
//! `repomap_token_budget > 0`.

use serde::Deserialize;
use serde_json::{Value, json};

use super::{Tool, ToolContext, ToolOutput};

pub struct RepomapTool;

impl RepomapTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Debug, Deserialize)]
struct RepomapInput {
    /// Repo-relative path prefixes personalizing the rank (files under
    /// discussion). Omit for the global importance map.
    #[serde(default)]
    seeds: Vec<String>,
    /// Token budget override for this call. Omit for the configured
    /// `repomap_token_budget`. 0 disables output for this call.
    #[serde(default)]
    budget: Option<usize>,
}

#[async_trait::async_trait]
impl Tool for RepomapTool {
    fn name(&self) -> &str {
        "repomap"
    }

    fn description(&self) -> &str {
        "Structural map of the repository: ranked files with symbol stubs (kind name:line), no bodies. Use to find what owns a symbol before reading files. Opt-in; absent when repomap_token_budget is 0."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "intent": super::intent_schema_property(),
                "seeds": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Path prefixes (files under discussion) or symbol names (identifiers under discussion) personalizing the rank. Omit for the global map."
                },
                "budget": {
                    "type": "integer",
                    "description": "Token budget override for this call. Omit for the configured repomap_token_budget."
                }
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput, anyhow::Error> {
        let input: RepomapInput = serde_json::from_value(input)?;
        let root = ctx
            .working_dir
            .clone()
            .ok_or_else(|| anyhow::anyhow!("repomap needs a working directory"))?;
        // Cold parse of a large tree blocks; keep it off the tokio worker.
        let seeds = input.seeds.clone();
        let budget = input.budget;
        let map = tokio::task::spawn_blocking(move || {
            let budget = budget.unwrap_or_else(jcode_base::repomap::token_budget_from_config);
            let seed_refs: Vec<&str> = seeds.iter().map(|s| s.as_str()).collect();
            jcode_base::repomap::build_map(&root, &seed_refs, budget)
        })
        .await
        .map_err(|err| anyhow::anyhow!("repomap worker failed: {err}"))?;
        match map {
            Some(text) => Ok(ToolOutput::new(text).with_title("repomap")),
            None => Ok(ToolOutput::new(
                "repomap produced no map (budget 0 or no symbols found).".to_string(),
            )
            .with_title("repomap")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_ctx(working_dir: Option<std::path::PathBuf>) -> ToolContext {
        ToolContext {
            session_id: "test-session".to_string(),
            message_id: "test-message".to_string(),
            tool_call_id: "test-tool-call".to_string(),
            working_dir,
            stdin_request_tx: None,
            graceful_shutdown_signal: None,
            execution_mode: crate::tool::ToolExecutionMode::Direct,
        }
    }

    #[tokio::test]
    async fn execute_returns_ranked_stubs_for_working_dir() {
        let dir = tempfile::tempdir().expect("temp");
        std::fs::write(dir.path().join("main.py"), "def main():\n    run()\n").unwrap();
        std::fs::write(dir.path().join("util.py"), "def run():\n    pass\n").unwrap();
        let tool = RepomapTool::new();
        let out = tool
            .execute(
                serde_json::json!({"seeds": [], "budget": 2000}),
                test_ctx(Some(dir.path().to_path_buf())),
            )
            .await
            .expect("execute");
        let text = format!("{:?}", out);
        assert!(text.contains("main.py"), "{text}");
        assert!(text.contains("util.py"), "{text}");
        assert!(text.contains("run"), "{text}");
    }

    #[tokio::test]
    async fn execute_budget_zero_reports_no_map() {
        let dir = tempfile::tempdir().expect("temp");
        std::fs::write(dir.path().join("a.py"), "def a():\n    pass\n").unwrap();
        let tool = RepomapTool::new();
        let out = tool
            .execute(
                serde_json::json!({"budget": 0}),
                test_ctx(Some(dir.path().to_path_buf())),
            )
            .await
            .expect("execute");
        assert!(format!("{:?}", out).contains("no map"), "{out:?}");
    }

    #[tokio::test]
    async fn execute_without_working_dir_errors() {
        let tool = RepomapTool::new();
        assert!(
            tool.execute(serde_json::json!({}), test_ctx(None))
                .await
                .is_err()
        );
    }
}
