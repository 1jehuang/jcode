//! Loader for rarely used built-in tools.
//!
//! Every tool definition is paid for on every request. Measured over 30 days,
//! the tools in [`RARELY_USED_TOOLS`] were each called in under 5% of sessions
//! yet together cost thousands of tokens per request. On providers with native
//! deferred loading their definitions stay out of the prompt until this tool
//! asks for them, which costs one extra call in the rare session that needs one.

use super::{Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use jcode_message_types::ToolDefinition;
use serde::Deserialize;
use serde_json::{Value, json};

pub(crate) const LOAD_TOOLS_NAME: &str = "load_tools";

/// Built-ins deferred when the provider supports it.
///
/// Not listed on purpose: `websearch` (providers swap it for their own server
/// tool by inspecting the eager list) and `desktop_selfdev` (only offered in
/// Desktop self-dev sessions, where it is the main tool).
pub(crate) const RARELY_USED_TOOLS: &[&str] = &[
    "browser",
    "calendar",
    "conversation_search",
    "gmail",
    "jcode_docs",
    "maintainer_feedback",
    "memory",
    "open",
    "panel",
    "schedule",
    "session_search",
    "skill_manage",
    "webfetch",
];

pub(crate) fn is_rarely_used(name: &str) -> bool {
    RARELY_USED_TOOLS.contains(&name)
}

/// Mark rarely used tools as deferred and add the loader that names them.
///
/// The loader is not model-visible in the registry, so surfaces that never
/// defer (and never call this) do not advertise a tool with nothing to load.
/// With `native` false, or `loader_allowed` false (allowlists, disabled
/// tools), nothing is deferred: a deferred tool the model cannot load would
/// simply be lost. `keep_eager` protects definitions that only share a name
/// with a built-in.
pub(crate) fn apply_deferral(
    tools: &mut Vec<ToolDefinition>,
    native: bool,
    loader_allowed: bool,
    keep_eager: impl Fn(&str) -> bool,
) {
    tools.retain(|tool| tool.name != LOAD_TOOLS_NAME);
    if !native || !loader_allowed {
        return;
    }
    let mut deferred: Vec<String> = Vec::new();
    for tool in tools.iter_mut() {
        if is_rarely_used(&tool.name) && !keep_eager(&tool.name) {
            tool.defer_loading = true;
            deferred.push(tool.name.clone());
        }
    }
    if deferred.is_empty() {
        return;
    }
    deferred.sort();
    let mut loader = LoadToolsTool::new().to_definition();
    loader.description = format!(
        "Load these tools before first use: {}.",
        deferred.join(", ")
    );
    tools.push(loader);
    // Deterministic order keeps the cached prefix stable.
    tools.sort_by(|a, b| a.name.cmp(&b.name));
}

pub struct LoadToolsTool;

impl LoadToolsTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Deserialize)]
struct LoadToolsInput {
    /// A list, or a single name: models often send one string for one tool.
    #[serde(default, alias = "tools", alias = "name")]
    names: Value,
}

#[async_trait]
impl Tool for LoadToolsTool {
    fn name(&self) -> &str {
        LOAD_TOOLS_NAME
    }

    fn model_visible(&self) -> bool {
        // Only `apply_deferral` advertises it, and only when something is deferred.
        false
    }

    fn description(&self) -> &str {
        // Replaced by `apply_deferral`, which names the tools actually deferred.
        "Load rarely used tools before first use."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["names"],
            "properties": {
                "intent": super::intent_schema_property(),
                "names": {"type": "array", "items": {"type": "string"}}
            }
        })
    }

    async fn execute(&self, input: Value, _ctx: ToolContext) -> Result<ToolOutput> {
        let params: LoadToolsInput = serde_json::from_value(input)?;
        let mut loaded: Vec<String> = Vec::new();
        let mut unknown: Vec<String> = Vec::new();
        let requested: Vec<&str> = match &params.names {
            Value::String(name) => name.split(',').collect(),
            Value::Array(names) => names.iter().filter_map(Value::as_str).collect(),
            _ => Vec::new(),
        };
        for name in requested {
            let name = name.trim().to_string();
            if name.is_empty() || loaded.contains(&name) || unknown.contains(&name) {
                continue;
            }
            if is_rarely_used(&name) {
                loaded.push(name);
            } else {
                unknown.push(name);
            }
        }
        anyhow::ensure!(
            !loaded.is_empty(),
            "No loadable tool named. Loadable: {}.",
            RARELY_USED_TOOLS.join(", ")
        );
        let mut output = format!("Loaded: {}. Call them directly now.", loaded.join(", "));
        if !unknown.is_empty() {
            output.push_str(&format!(
                "\nNot loadable (already available or unknown): {}.",
                unknown.join(", ")
            ));
        }
        Ok(ToolOutput::new(output)
            .with_title(format!("Loaded {}", loaded.join(", ")))
            .with_metadata(json!({ "tool_references": loaded })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def(name: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.to_string(),
            description: format!("{name} description"),
            input_schema: json!({"type": "object", "properties": {}}),
            defer_loading: false,
        }
    }

    fn names(tools: &[ToolDefinition], deferred: bool) -> Vec<&str> {
        tools
            .iter()
            .filter(|tool| tool.defer_loading == deferred)
            .map(|tool| tool.name.as_str())
            .collect()
    }

    #[test]
    fn defers_rare_tools_and_names_them_in_the_loader() {
        let mut tools = vec![def("bash"), def("gmail"), def("browser")];
        apply_deferral(&mut tools, true, true, |_| false);
        assert_eq!(names(&tools, true), ["browser", "gmail"]);
        assert_eq!(names(&tools, false), ["bash", LOAD_TOOLS_NAME]);
        let loader = tools.iter().find(|t| t.name == LOAD_TOOLS_NAME).unwrap();
        assert_eq!(
            loader.description,
            "Load these tools before first use: browser, gmail."
        );
        assert_eq!(loader.input_schema["required"], json!(["names", "intent"]));
    }

    #[test]
    fn without_native_support_everything_stays_eager_and_there_is_no_loader() {
        let mut tools = vec![def("bash"), def("gmail")];
        apply_deferral(&mut tools, false, true, |_| false);
        assert_eq!(names(&tools, false), ["bash", "gmail"]);
        assert!(names(&tools, true).is_empty());
    }

    #[test]
    fn nothing_is_deferred_when_the_loader_is_not_allowed() {
        // An allowlist that omits the loader must not strand the tools it kept.
        let mut tools = vec![def("bash"), def("gmail")];
        apply_deferral(&mut tools, true, false, |_| false);
        assert_eq!(names(&tools, false), ["bash", "gmail"]);
    }

    #[test]
    fn protected_names_stay_eager_and_an_unneeded_loader_is_not_added() {
        let mut tools = vec![def("gmail")];
        apply_deferral(&mut tools, true, true, |name| name == "gmail");
        assert_eq!(names(&tools, false), ["gmail"]);
    }

    #[test]
    fn the_loader_costs_less_than_the_cheapest_tool_it_defers() {
        let mut tools: Vec<ToolDefinition> = RARELY_USED_TOOLS.iter().map(|n| def(n)).collect();
        apply_deferral(&mut tools, true, true, |_| false);
        let loader = tools.iter().find(|t| t.name == LOAD_TOOLS_NAME).unwrap();
        let text = format!("{}{}", loader.description, loader.input_schema);
        assert!(text.len() < 400, "loader is {} chars: {text}", text.len());
    }

    #[tokio::test]
    async fn loading_returns_references_for_known_names_only() {
        let ctx = ToolContext {
            session_id: "load-tools-test".to_string(),
            message_id: "m".to_string(),
            tool_call_id: "c".to_string(),
            working_dir: None,
            stdin_request_tx: None,
            graceful_shutdown_signal: None,
            execution_mode: super::super::ToolExecutionMode::Direct,
        };
        let output = LoadToolsTool::new()
            .execute(json!({"names": ["gmail", "bash", "gmail"]}), ctx.clone())
            .await
            .unwrap();
        assert_eq!(
            output.metadata.unwrap()["tool_references"],
            json!(["gmail"])
        );
        assert!(output.output.contains("Not loadable"));
        assert!(
            LoadToolsTool::new()
                .execute(json!({"names": ["bash"]}), ctx)
                .await
                .is_err()
        );
    }
}
