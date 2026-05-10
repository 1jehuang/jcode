//! # Hooks 自动化系统
//!
//! 从 Claude Code 移植的钩子系统

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::debug;

/// 钩子事件类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HookEvent {
    PreToolUse,
    PostToolUse,
    PostToolUseFailure,
    UserPromptSubmit,
    SessionStart,
    SessionEnd,
    FileChanged,
    BeforeBuild,
    AfterBuild,
    OnGitCommit,
    OnTestComplete,
    BeforeToolExecution,
}

/// 钩子动作类型
#[derive(Debug, Clone)]
pub enum HookAction {
    Command { command: String, timeout_secs: u64, async_exec: bool },
    Http { url: String, headers: HashMap<String, String>, timeout_secs: u64 },
    Prompt { prompt: String, model: Option<String> },
}

/// 钩子匹配器
#[derive(Debug, Clone)]
pub struct HookMatcher {
    pub name: String,
    pub actions: Vec<HookAction>,
    pub if_condition: Option<String>,
    pub once: bool,
    pub already_fired: bool,
}

/// 钩子系统
pub struct HookSystem {
    triggers: Arc<RwLock<HashMap<HookEvent, Vec<HookMatcher>>>>,
}

impl HookSystem {
    pub fn new() -> Self {
        Self { triggers: Arc::new(RwLock::new(HashMap::new())) }
    }

    /// 注册钩子
    pub async fn register(&self, event: HookEvent, matcher: HookMatcher) {
        self.triggers.write().await.entry(event).or_default().push(matcher);
    }

    /// 触发执行
    pub async fn trigger(&self, event: HookEvent) -> Vec<String> {
        let mut results = Vec::new();
        let matchers = self.triggers.read().await.get(&event).cloned().unwrap_or_default();

        for matcher in matchers {
            if matcher.once && matcher.already_fired { continue; }
            results.push(matcher.name.clone());
            debug!("Hook triggered: {} for event {:?}", matcher.name, event);
        }
        results
    }
}

impl Default for HookSystem { fn default() -> Self { Self::new() } }
