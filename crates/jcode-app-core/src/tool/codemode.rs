//! Opt-in `codemode` tool: run model-written JavaScript in an embedded QuickJS
//! sandbox that can call any tool available to the session.
//!
//! Enabled with `[tools] codemode = true` (or `JCODE_CODEMODE=1`). See
//! `jcode-codemode` for the sandbox itself. This module is the harness side:
//! it lists the session's tools, routes calls through [`Registry::execute`]
//! (so session policy, hooks, and MCP routing all apply), and persists the
//! script `store()` per session between runs.

use super::{Registry, Tool, ToolContext, ToolOutput, WeakRegistry};
use anyhow::Result;
use async_trait::async_trait;
use futures::future::BoxFuture;
use jcode_codemode::{Host, Limits, ToolInfo};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// Tools never reachable from inside a script: itself (no recursion) and
/// `batch` (pointless inside a script that already has `Promise.all`).
const EXCLUDED: &[&str] = &["codemode", "batch"];

/// Maximum characters of the final report returned to the model.
const MAX_REPORT_CHARS: usize = 60_000;

const DESCRIPTION: &str = r#"Run a JavaScript program in a sandbox that can call any of your other tools, including MCP tools. Use it to chain, loop over, filter, or parallelize many tool calls in one step, so intermediate results never enter the conversation. Only what you `return` (plus console output) comes back.

The code is the body of an async function: use `await` and `return`.
- `await tools.<name>(args)` calls a tool with the same arguments it normally takes. JSON output is parsed into objects, other output is a string. Failures throw an Error.
- `await callTool(name, args)` does the same for names that are not valid identifiers.
- `listTools(filter?)` returns `[{name, description}]`. `describeTool(name)` returns the full description and input_schema. Call these first if unsure of a tool's arguments.
- `store(key, value)` / `load(key)` / `storedKeys()` keep JSON values across codemode calls in this session. Use them to avoid refetching.
- `console.log(...)` output is returned along with the result.
- Run independent calls in parallel with `Promise.all`.
No filesystem, network, timers, or modules exist except through tools. Limits: 64 MB heap, 300 s, 2000 tool calls per run.

Example:
const files = (await tools.bash({ command: "git ls-files '*.rs'" })).trim().split("\n");
const hits = await Promise.all(files.slice(0, 50).map(async (f) => {
  const text = await tools.read({ file_path: f, limit: 400 });
  return { f, todos: (text.match(/TODO/g) || []).length };
}));
store("todo_counts", hits);
return hits.filter((h) => h.todos > 0).sort((a, b) => b.todos - a.todos).slice(0, 10);"#;

pub struct CodemodeTool {
    registry: WeakRegistry,
}

impl CodemodeTool {
    pub(super) fn new(registry: WeakRegistry) -> Self {
        Self { registry }
    }
}

#[derive(Deserialize)]
struct CodemodeInput {
    #[serde(alias = "script", alias = "source", alias = "js")]
    code: String,
}

struct RegistryHost {
    registry: Registry,
    ctx: ToolContext,
    counter: AtomicUsize,
}

impl Host for RegistryHost {
    fn call_tool(&self, name: String, args: Value) -> BoxFuture<'static, Result<Value, String>> {
        let n = self.counter.fetch_add(1, Ordering::SeqCst) + 1;
        let registry = self.registry.clone();
        let sub_ctx = self.ctx.for_subcall(format!("codemode-{n}-{name}"));
        Box::pin(async move {
            let args = match args {
                Value::Object(map) => Value::Object(map),
                other => return Err(format!("arguments must be an object, got {other}")),
            };
            match registry.execute(&name, args, sub_ctx).await {
                Ok(output) => Ok(jcode_codemode::decode_tool_output(&output.output)),
                Err(err) => Err(crate::util::format_error_chain(&err)),
            }
        })
    }
}

/// Whether the opt-in codemode tool is enabled in config.
pub(crate) fn enabled() -> bool {
    jcode_codemode::AVAILABLE && crate::config::config().tools.codemode
}

fn store_path(session_id: &str) -> Option<PathBuf> {
    let safe: String = session_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if safe.is_empty() {
        return None;
    }
    Some(
        crate::storage::jcode_dir()
            .ok()?
            .join("codemode")
            .join(format!("{safe}.json")),
    )
}

fn load_store(session_id: &str) -> Map<String, Value> {
    store_path(session_id)
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str::<Map<String, Value>>(&text).ok())
        .unwrap_or_default()
}

fn save_store(session_id: &str, store: &Map<String, Value>) {
    let Some(path) = store_path(session_id) else {
        return;
    };
    if store.is_empty() {
        let _ = std::fs::remove_file(&path);
        return;
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, Value::Object(store.clone()).to_string()).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

fn truncate(text: String, max: usize) -> String {
    if text.chars().count() <= max {
        return text;
    }
    let cut: String = text.chars().take(max).collect();
    format!("{cut}\n...(truncated; return a smaller value or store() the rest)")
}

pub(crate) fn render_report(out: &jcode_codemode::RunOutput) -> String {
    let mut report = String::new();
    if !out.logs.is_empty() {
        report.push_str("console:\n");
        for line in &out.logs {
            report.push_str(line);
            report.push('\n');
        }
        if out.logs_truncated {
            report.push_str("...(console output truncated)\n");
        }
        report.push('\n');
    }
    match (&out.error, &out.result) {
        (Some(err), _) => {
            report.push_str("error: ");
            report.push_str(err);
            report.push('\n');
        }
        (None, Some(value)) => {
            report.push_str("result:\n");
            let rendered = match value {
                Value::String(s) => s.clone(),
                other => serde_json::to_string_pretty(other).unwrap_or_else(|_| other.to_string()),
            };
            report.push_str(&rendered);
            report.push('\n');
        }
        (None, None) => report.push_str("result: undefined\n"),
    }
    let mut footer = format!(
        "\n[{} tool call{}, {:.1}s",
        out.tool_calls,
        if out.tool_calls == 1 { "" } else { "s" },
        out.elapsed.as_secs_f64()
    );
    if !out.store.is_empty() {
        let keys: Vec<&str> = out.store.keys().map(String::as_str).take(20).collect();
        footer.push_str(&format!(", stored: {}", keys.join(", ")));
    }
    footer.push(']');
    report.push_str(&footer);
    truncate(report, MAX_REPORT_CHARS)
}

#[async_trait]
impl Tool for CodemodeTool {
    fn name(&self) -> &str {
        "codemode"
    }

    fn description(&self) -> &str {
        DESCRIPTION
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["code"],
            "properties": {
                "intent": super::intent_schema_property(),
                "code": {
                    "type": "string",
                    "description": "Body of an async JavaScript function. Use await and return."
                }
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let registry = self
            .registry
            .upgrade()
            .ok_or_else(|| anyhow::anyhow!("codemode registry is no longer available"))?;
        let params: CodemodeInput = serde_json::from_value(input)?;
        if params.code.trim().is_empty() {
            anyhow::bail!("code is empty");
        }

        let tools: Vec<ToolInfo> = registry
            .definitions_for_session(&ctx.session_id)
            .await
            .into_iter()
            .filter(|def| !EXCLUDED.contains(&def.name.as_str()))
            .map(|def| ToolInfo {
                name: def.name,
                description: def.description,
                input_schema: def.input_schema,
            })
            .collect();

        let host = Arc::new(RegistryHost {
            registry,
            ctx: ctx.clone(),
            counter: AtomicUsize::new(0),
        });
        let store = load_store(&ctx.session_id);
        let limits = Limits {
            timeout: Duration::from_secs(300),
            ..Limits::default()
        };
        let out = jcode_codemode::run(params.code, host, tools, store, limits).await;
        save_store(&ctx.session_id, &out.store);

        crate::logging::info(&format!(
            "[tool:codemode] session={} calls={} elapsed_ms={} error={}",
            ctx.session_id,
            out.tool_calls,
            out.elapsed.as_millis(),
            out.error.as_deref().unwrap_or("none")
        ));

        let report = render_report(&out);
        if out.error.is_some() && out.tool_calls == 0 && out.logs.is_empty() {
            // Nothing happened: surface as a tool error so the model retries.
            anyhow::bail!("{report}");
        }
        Ok(ToolOutput::new(report).with_title(format!(
            "codemode: {} call{}",
            out.tool_calls,
            if out.tool_calls == 1 { "" } else { "s" }
        )))
    }
}

#[cfg(test)]
#[path = "codemode_tests.rs"]
mod codemode_tests;
