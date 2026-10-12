use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

struct AccountAwareTool(Arc<AtomicBool>);

#[async_trait]
impl crate::tool::Tool for AccountAwareTool {
    fn name(&self) -> &str {
        "compile_remote"
    }
    fn description(&self) -> &str {
        if self.0.load(Ordering::SeqCst) {
            "Subscription verified. Spend shared cloud credits to compile."
        } else {
            "Subscribe to Jcode and sign in to compile remotely."
        }
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","properties":{}})
    }
    async fn execute(
        &self,
        _: serde_json::Value,
        _: crate::tool::ToolContext,
    ) -> Result<ToolOutput> {
        unreachable!("schema test never executes compute")
    }
}

/// Tool definitions are part of the provider prompt-cache prefix. Once a
/// snapshot is locked, a tool whose description depends on runtime state (such
/// as account status) must not rewrite it mid-session.
#[tokio::test]
async fn locked_snapshots_ignore_runtime_description_changes() {
    let _sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    for mode in [
        crate::config::McpToolsMode::Eager,
        crate::config::McpToolsMode::Deferred,
    ] {
        let paid = Arc::new(AtomicBool::new(false));
        let registry = Registry::empty();
        registry
            .register(
                "compile_remote".into(),
                Arc::new(AccountAwareTool(paid.clone())),
            )
            .await;
        let provider: Arc<dyn Provider> = Arc::new(NativeAutoCompactionProvider);
        let mut agent = Agent::new(provider, registry);
        agent.mcp_tools_mode = mode;
        agent.allowed_tools = Some(HashSet::from(["compile_remote".into()]));
        let before = agent.tool_definitions().await;
        assert_eq!(before.len(), 1);
        agent.mcp_late_register_resolved = true;
        paid.store(true, Ordering::SeqCst);
        let after = agent.tool_definitions().await;
        assert_eq!(after[0].description, before[0].description);
        assert_eq!(after[0].input_schema, before[0].input_schema);
    }
}
