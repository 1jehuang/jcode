use anyhow::Result;
use clap::Parser;
use jcode::id::new_id;
use jcode::message::{Message, ToolDefinition};
use jcode::provider::{EventStream, Provider};
use jcode::tool::{Registry, ToolContext};
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Parser)]
#[command(name = "jcode-harness")]
#[command(about = "Run a deterministic tool harness smoke test")]
struct Args {
    /// Use an explicit working directory (defaults to a temp folder).
    #[arg(long)]
    cwd: Option<String>,

    /// Include network-backed tools (webfetch/websearch/codesearch).
    #[arg(long)]
    include_network: bool,
}

struct NoopProvider;

#[async_trait::async_trait]
impl Provider for NoopProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        anyhow::bail!("Noop provider - tool harness does not invoke models.")
    }

    fn name(&self) -> &str {
        "noop"
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(NoopProvider)
    }

    fn available_models_display(&self) -> Vec<String> {
        vec![]
    }

    async fn prefetch_models(&self) -> Result<()> {
        Ok(())
    }
}

struct ToolCase {
    name: &'static str,
    input: serde_json::Value,
    label: &'static str,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let workspace = if let Some(cwd) = args.cwd {
        PathBuf::from(cwd)
    } else {
        create_temp_workspace()?
    };

    std::fs::create_dir_all(&workspace)?;
    std::env::set_current_dir(&workspace)?;
    eprintln!("Harness workspace: {}", workspace.display());

    let provider: Arc<dyn Provider> = Arc::new(NoopProvider);
    let registry = Registry::new(provider).await;

    let session_id = new_id("harness");
    let base_ctx = ToolContext {
        session_id: session_id.clone(),
        message_id: session_id.clone(),
        tool_call_id: String::new(),
        working_dir: Some(workspace.clone()),
        stdin_request_tx: None,
    };

    let mut cases = Vec::new();
    cases.push(ToolCase {
        name: "write",
        label: "write sample.txt",
        input: json!({"file_path": "sample.txt", "content": "alpha\nbeta\n"}),
    });
    cases.push(ToolCase {
        name: "read",
        label: "read sample.txt",
        input: json!({"file_path": "sample.txt"}),
    });
    cases.push(ToolCase {
        name: "edit",
        label: "edit sample.txt (alpha -> alpha1)",
        input: json!({"file_path": "sample.txt", "old_string": "alpha", "new_string": "alpha1"}),
    });
    cases.push(ToolCase {
        name: "multiedit",
        label: "multiedit sample.txt",
        input: json!({
            "file_path": "sample.txt",
            "edits": [
                {"old_string": "alpha1", "new_string": "alpha2"},
                {"old_string": "beta", "new_string": "beta1"}
            ]
        }),
    });
    cases.push(ToolCase {
        name: "patch",
        label: "patch sample.txt",
        input: json!({"patch_text": "--- a/sample.txt\n+++ b/sample.txt\n@@ -1,2 +1,3 @@\n alpha2\n beta1\n+gamma\n"}),
    });
    cases.push(ToolCase {
        name: "apply_patch",
        label: "apply_patch add file",
        input: json!({"patch_text": "*** Begin Patch\n*** Add File: added.txt\n+added\n*** End Patch\n"}),
    });
    cases.push(ToolCase {
        name: "ls",
        label: "ls .",
        input: json!({"path": "."}),
    });
    cases.push(ToolCase {
        name: "glob",
        label: "glob *.txt",
        input: json!({"pattern": "*.txt"}),
    });
    cases.push(ToolCase {
        name: "grep",
        label: "grep gamma",
        input: json!({"pattern": "gamma", "path": "."}),
    });
    cases.push(ToolCase {
        name: "bash",
        label: "bash pwd",
        input: json!({"command": "pwd"}),
    });
    cases.push(ToolCase {
        name: "invalid",
        label: "invalid tool call",
        input: json!({"tool": "unknown", "error": "missing required field"}),
    });
    cases.push(ToolCase {
        name: "todowrite",
        label: "todowrite",
        input: json!({"todos": [{"content": "harness task", "status": "pending", "priority": "low", "id": "1"}]}),
    });
    cases.push(ToolCase {
        name: "todoread",
        label: "todoread",
        input: json!({}),
    });
    cases.push(ToolCase {
        name: "batch",
        label: "batch ls + read",
        input: json!({
            "tool_calls": [
                {"tool": "ls", "parameters": {"path": "."}},
                {"tool": "read", "parameters": {"file_path": "sample.txt"}}
            ]
        }),
    });

    if args.include_network {
        cases.push(ToolCase {
            name: "webfetch",
            label: "webfetch example.com",
            input: json!({"url": "https://example.com", "format": "text"}),
        });
        cases.push(ToolCase {
            name: "websearch",
            label: "websearch rust async",
            input: json!({"query": "rust async await"}),
        });
        cases.push(ToolCase {
            name: "codesearch",
            label: "codesearch tokio spawn",
            input: json!({"query": "tokio::spawn"}),
        });
    }

    for (idx, case) in cases.iter().enumerate() {
        let ctx = ToolContext {
            tool_call_id: format!("harness-{}", idx + 1),
            ..base_ctx.clone()
        };
        println!("\n== {} ({}) ==", case.name, case.label);
        match registry.execute(case.name, case.input.clone(), ctx).await {
            Ok(output) => {
                if let Some(title) = output.title {
                    println!("[title] {}", title);
                }
                println!("{}", output.output);
            }
            Err(err) => {
                println!("[error] {}", err);
            }
        }
    }

    Ok(())
}

fn create_temp_workspace() -> Result<PathBuf> {
    let mut path = std::env::temp_dir();
    path.push(format!("jcode-harness-{}", new_id("run")));
    Ok(path)
}
