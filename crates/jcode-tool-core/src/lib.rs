use anyhow::Result;
use async_trait::async_trait;
use jcode_agent_runtime::InterruptSignal;
use jcode_message_types::ToolDefinition;
use jcode_tool_types::ToolOutput;
use serde_json::Value;
use std::path::{Path, PathBuf};

pub const TOOL_INTENT_DESCRIPTION: &str = concat!(
    "Short natural-language label explaining why this tool call is being made. ",
    "Used for compact UI display only. Optional; do not use this instead of required tool parameters."
);

pub fn intent_schema_property() -> Value {
    serde_json::json!({
        "type": "string",
        "description": TOOL_INTENT_DESCRIPTION,
    })
}

/// A request for stdin input from a running command.
pub struct StdinInputRequest {
    pub request_id: String,
    pub prompt: String,
    pub is_password: bool,
    pub response_tx: tokio::sync::oneshot::Sender<String>,
}

#[derive(Clone)]
pub struct ToolContext {
    pub session_id: String,
    pub message_id: String,
    pub tool_call_id: String,
    pub working_dir: Option<PathBuf>,
    /// Optional sandbox root. When `Some`, every path that flows through
    /// [`ToolContext::resolve_path_checked`] must canonicalize to a
    /// location inside this directory; otherwise the call is rejected.
    /// Configured via `--sandbox-root <DIR>` CLI flag or
    /// `JCODE_SANDBOX_ROOT` env var (issue #110).
    pub sandbox_root: Option<PathBuf>,
    pub stdin_request_tx: Option<tokio::sync::mpsc::UnboundedSender<StdinInputRequest>>,
    pub graceful_shutdown_signal: Option<InterruptSignal>,
    pub execution_mode: ToolExecutionMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolExecutionMode {
    AgentTurn,
    Direct,
}

impl ToolContext {
    pub fn for_subcall(&self, tool_call_id: String) -> Self {
        Self {
            session_id: self.session_id.clone(),
            message_id: self.message_id.clone(),
            tool_call_id,
            working_dir: self.working_dir.clone(),
            sandbox_root: self.sandbox_root.clone(),
            stdin_request_tx: self.stdin_request_tx.clone(),
            graceful_shutdown_signal: self.graceful_shutdown_signal.clone(),
            execution_mode: self.execution_mode,
        }
    }

    pub fn resolve_path(&self, path: &Path) -> PathBuf {
        if path.is_absolute() {
            path.to_path_buf()
        } else if let Some(ref base) = self.working_dir {
            base.join(path)
        } else {
            path.to_path_buf()
        }
    }

    /// Resolve a path AND enforce the sandbox if one is configured.
    ///
    /// Returns `Err` if the resolved path escapes `sandbox_root`. Symlink
    /// traversal is blocked by canonicalizing both sides before comparison;
    /// when the target does not yet exist (e.g. a new file write), we walk
    /// up to the nearest existing ancestor and canonicalize that instead.
    pub fn resolve_path_checked(&self, path: &Path) -> anyhow::Result<PathBuf> {
        let resolved = self.resolve_path(path);
        let Some(ref root) = self.sandbox_root else {
            return Ok(resolved);
        };

        let canonical_root = root.canonicalize().unwrap_or_else(|_| root.clone());
        let canonical_target = canonicalize_existing_ancestor(&resolved);

        if canonical_target.starts_with(&canonical_root) {
            Ok(resolved)
        } else {
            Err(anyhow::anyhow!(
                "sandbox violation: path {} is outside the configured sandbox root {}",
                resolved.display(),
                canonical_root.display(),
            ))
        }
    }
}

/// Canonicalize the closest existing ancestor of `target`, then re-append
/// the missing tail components. Lets us check sandbox containment for
/// paths that don't exist yet (new file writes).
fn canonicalize_existing_ancestor(target: &Path) -> PathBuf {
    let mut current = target.to_path_buf();
    let mut tail = std::collections::VecDeque::new();
    loop {
        if let Ok(canonical) = current.canonicalize() {
            let mut out = canonical;
            for segment in tail.iter() {
                out.push(segment);
            }
            return out;
        }
        match current.file_name().map(|s| s.to_owned()) {
            Some(name) => {
                tail.push_front(name);
                if !current.pop() {
                    return target.to_path_buf();
                }
            }
            None => return target.to_path_buf(),
        }
    }
}

/// A tool that can be executed by the agent.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Tool name (must match what's sent to the API).
    fn name(&self) -> &str;

    /// Human-readable description.
    fn description(&self) -> &str;

    /// JSON Schema for the input parameters.
    fn parameters_schema(&self) -> Value;

    /// Execute the tool with the given input.
    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput>;

    /// Convert to API tool definition.
    fn to_definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: self.description().to_string(),
            input_schema: self.parameters_schema(),
        }
    }
}

#[cfg(test)]
mod sandbox_tests {
    use super::*;
    use std::fs;

    fn ctx_with_sandbox(working_dir: PathBuf, sandbox_root: PathBuf) -> ToolContext {
        ToolContext {
            session_id: "test".into(),
            message_id: "msg".into(),
            tool_call_id: "call".into(),
            working_dir: Some(working_dir),
            sandbox_root: Some(sandbox_root),
            stdin_request_tx: None,
            graceful_shutdown_signal: None,
            execution_mode: ToolExecutionMode::Direct,
        }
    }

    fn ctx_without_sandbox(working_dir: PathBuf) -> ToolContext {
        ToolContext {
            session_id: "test".into(),
            message_id: "msg".into(),
            tool_call_id: "call".into(),
            working_dir: Some(working_dir),
            sandbox_root: None,
            stdin_request_tx: None,
            graceful_shutdown_signal: None,
            execution_mode: ToolExecutionMode::Direct,
        }
    }

    #[test]
    fn allows_relative_path_inside_sandbox() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        fs::write(root.join("inside.txt"), "hi").unwrap();
        let ctx = ctx_with_sandbox(root.clone(), root);
        let resolved = ctx
            .resolve_path_checked(Path::new("inside.txt"))
            .expect("inside-sandbox path must resolve");
        assert!(resolved.ends_with("inside.txt"));
    }

    #[test]
    fn rejects_absolute_path_outside_sandbox() {
        let dir = tempfile::TempDir::new().unwrap();
        let other = tempfile::TempDir::new().unwrap();
        let ctx = ctx_with_sandbox(dir.path().to_path_buf(), dir.path().to_path_buf());
        let outside = other.path().join("evil.txt");
        let err = ctx.resolve_path_checked(&outside).unwrap_err();
        assert!(err.to_string().contains("sandbox violation"), "got: {err}");
    }

    #[test]
    fn rejects_dot_dot_escape() {
        let dir = tempfile::TempDir::new().unwrap();
        let inner = dir.path().join("nested");
        fs::create_dir(&inner).unwrap();
        let ctx = ctx_with_sandbox(inner.clone(), inner);
        let err = ctx
            .resolve_path_checked(Path::new("../../etc/passwd"))
            .unwrap_err();
        assert!(err.to_string().contains("sandbox violation"));
    }

    #[test]
    fn no_sandbox_passes_through() {
        let dir = tempfile::TempDir::new().unwrap();
        let ctx = ctx_without_sandbox(dir.path().to_path_buf());
        // Even an absolute path elsewhere is fine when no sandbox configured.
        let outside = std::env::temp_dir();
        let resolved = ctx.resolve_path_checked(&outside).unwrap();
        assert_eq!(resolved, outside);
    }

    #[test]
    fn allows_new_file_inside_sandbox() {
        let dir = tempfile::TempDir::new().unwrap();
        let ctx = ctx_with_sandbox(dir.path().to_path_buf(), dir.path().to_path_buf());
        // File doesn't exist yet — resolve_path_checked must still accept
        // it because parent dir is inside sandbox.
        let resolved = ctx.resolve_path_checked(Path::new("new-file.txt")).unwrap();
        assert!(resolved.ends_with("new-file.txt"));
    }
}
