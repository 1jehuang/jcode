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
    /// `resolve_path_checked` must canonicalize to a location inside this
    /// directory; otherwise the call is rejected. This is the file-system
    /// confinement requested by `--sandbox` / `JCODE_SANDBOX_ROOT`.
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

#[cfg(test)]
mod sandbox_tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn ctx_with_sandbox(working_dir: PathBuf, sandbox: PathBuf) -> ToolContext {
        ToolContext {
            session_id: "t".into(),
            message_id: "t".into(),
            tool_call_id: "t".into(),
            working_dir: Some(working_dir),
            sandbox_root: Some(sandbox),
            stdin_request_tx: None,
            graceful_shutdown_signal: None,
            execution_mode: ToolExecutionMode::Direct,
        }
    }

    #[test]
    fn allows_relative_paths_inside_sandbox() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        let ctx = ctx_with_sandbox(root.clone(), root.clone());

        let resolved = ctx
            .resolve_path_checked(Path::new("subdir/file.txt"))
            .expect("inside sandbox should be allowed");
        assert!(resolved.starts_with(&root));
    }

    #[test]
    fn rejects_absolute_paths_outside_sandbox() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        let ctx = ctx_with_sandbox(root.clone(), root);

        let err = ctx
            .resolve_path_checked(Path::new("/etc/passwd"))
            .expect_err("outside sandbox should fail");
        assert!(err.to_string().contains("sandbox violation"));
    }

    #[test]
    fn rejects_dotdot_escapes() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("inside");
        fs::create_dir_all(&root).unwrap();
        let ctx = ctx_with_sandbox(root.clone(), root);

        let err = ctx
            .resolve_path_checked(Path::new("../sibling"))
            .expect_err("../ escape should fail");
        assert!(err.to_string().contains("sandbox violation"));
    }

    #[test]
    fn no_sandbox_means_no_check() {
        let tmp = TempDir::new().unwrap();
        let ctx = ToolContext {
            session_id: "t".into(),
            message_id: "t".into(),
            tool_call_id: "t".into(),
            working_dir: Some(tmp.path().to_path_buf()),
            sandbox_root: None,
            stdin_request_tx: None,
            graceful_shutdown_signal: None,
            execution_mode: ToolExecutionMode::Direct,
        };
        // /etc/passwd is allowed when no sandbox is set: behavior is identical
        // to the legacy resolve_path so existing call sites stay sound.
        let resolved = ctx
            .resolve_path_checked(Path::new("/etc/passwd"))
            .expect("no sandbox = no check");
        assert_eq!(resolved, PathBuf::from("/etc/passwd"));
    }

    #[test]
    fn allows_new_file_inside_sandbox() {
        // The target may not exist (write of a brand-new file). We must still
        // accept it when the parent canonicalizes inside the sandbox.
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        let ctx = ctx_with_sandbox(root.clone(), root);
        let resolved = ctx
            .resolve_path_checked(Path::new("new/nested/file.rs"))
            .expect("new file under sandbox should be allowed");
        assert!(resolved.ends_with("new/nested/file.rs"));
    }
}

/// Canonicalize `path` if it exists; otherwise canonicalize the closest
/// existing ancestor and re-attach the missing tail. This lets us validate
/// a write target that does not yet exist while still resolving any symlinks
/// in the parents (which is what defeats `/sandbox/escape -> /etc` tricks).
fn canonicalize_existing_ancestor(path: &Path) -> PathBuf {
    if let Ok(c) = path.canonicalize() {
        return c;
    }
    let mut current = path.to_path_buf();
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    while !current.exists() {
        match (current.file_name(), current.parent()) {
            (Some(name), Some(parent)) => {
                tail.push(name.to_os_string());
                current = parent.to_path_buf();
            }
            _ => break,
        }
    }
    let mut canonical = current.canonicalize().unwrap_or(current);
    for name in tail.into_iter().rev() {
        canonical.push(name);
    }
    canonical
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
