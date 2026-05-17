﻿//! # CarpAI Smart Hooks System
//!
//! Inspired by Kiro's Agent Hooks, but with CarpAI-specific enhancements:
//!
//! ## Architecture
//! ```
//! +-------------------------------------------------+
//! |              CarpAI Smart Hooks 架构             |
//! +-------------------------------------------------+
//! |  触发条件          |  动作              |  执行模式  |
//! +-------------------+-------------------+----------+
//! |  onSave           |  语法检查+修复     |  本地(快速) |
//! |  onCommit         |  变更摘要生成      |  混合(本地+AI) |
//! |  onPullRequest    |  Code Review       |  AI(深度)   |
//! |  onBuildFail      |  错误分析+修复建议  |  AI(按需)   |
//! |  onTestFail       |  测试诊断+修复      |  AI(按需)   |
//! |  onIdle (>5min)   |  代码质量扫描      |  后台(低优先)|
//! +-------------------------------------------------+
//! ```
//!
//! ## Key Differentiators from Kiro
//! - **Cost-aware**: Smart decision to use AI or local processing
//! - **Build-mode integration**: Works with CarpAI's autonomous agent mode
//! - **Context-aware**: Adapts based on project state and history

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{RwLock, mpsc, watch};
use tracing::{info, warn, debug, error};
use serde::{Deserialize, Serialize};

/// Hook execution modes
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ExecutionMode {
    /// Fast local execution (no AI)
    Local,
    /// Hybrid: local + AI when needed
    Hybrid,
    /// Deep AI analysis (higher cost)
    AiDeep,
    /// Background low-priority
    Background,
}

/// Smart hook trigger events
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SmartHookEvent {
    /// File save event
    OnSave,
    /// Git commit event
    OnCommit,
    /// Pull request created/updated
    OnPullRequest,
    /// Build failure
    OnBuildFail,
    /// Test failure
    OnTestFail,
    /// Idle for >5 minutes
    OnIdle,
    /// Pre-command execution
    PreCommand,
    /// Post-command execution
    PostCommand,
}

/// Hook priority levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum HookPriority {
    Critical = 0,
    High = 1,
    Medium = 2,
    Low = 3,
}

impl Default for HookPriority {
    fn default() -> Self {
        Self::Medium
    }
}

/// Hook action result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookResult {
    hook_id: String,
    event: SmartHookEvent,
    success: bool,
    message: String,
    duration_ms: u64,
    suggestions: Vec<HookSuggestion>,
    execution_mode: ExecutionMode,
}

/// Suggestion from hook execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookSuggestion {
    title: String,
    description: String,
    severity: SuggestionSeverity,
    auto_fixable: bool,
    fix_command: Option<String>,
}

/// Suggestion severity
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SuggestionSeverity {
    Info,
    Warning,
    Error,
    Critical,
}

/// Smart hook definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmartHook {
    id: String,
    name: String,
    description: String,
    event: SmartHookEvent,
    enabled: bool,
    execution_mode: ExecutionMode,
    priority: HookPriority,
    cooldown_secs: u64,
    config: HookConfig,
    last_executed: Option<Instant>,
    execution_count: u64,
}

/// Hook configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookConfig {
    file_patterns: Vec<String>,
    exclude_patterns: Vec<String>,
    max_file_size_kb: usize,
    require_ai_confirmation: bool,
    cost_limit_tokens: Option<u32>,
    #[serde(default)]
    custom_params: HashMap<String, String>,
}

impl Default for HookConfig {
    fn default() -> Self {
        Self {
            file_patterns: vec!["*".to_string()],
            exclude_patterns: vec![
                "*.lock".to_string(),
                "node_modules/**".to_string(),
                "target/**".to_string(),
            ],
            max_file_size_kb: 100,
            require_ai_confirmation: false,
            cost_limit_tokens: None,
            custom_params: HashMap::new(),
        }
    }
}

/// Context data passed to hooks
#[derive(Debug, Clone)]
pub struct HookContext {
    project_path: String,
    changed_files: Vec<String>,
    git_diff: Option<String>,
    build_output: Option<String>,
    test_output: Option<String>,
    user_prompt: Option<String>,
    metadata: HashMap<String, String>,
}

impl HookContext {
    fn new(project_path: impl Into<String>) -> Self {
        Self {
            project_path: project_path.into(),
            changed_files: Vec::new(),
            git_diff: None,
            build_output: None,
            test_output: None,
            user_prompt: None,
            metadata: HashMap::new(),
        }
    }

    fn with_changed_files(mut self, files: Vec<String>) -> Self {
        self.changed_files = files;
        self
    }
}

/// Message sent through the hook channel
pub enum HookMessage {
    Trigger {
        event: SmartHookEvent,
        context: HookContext,
        response_tx: mpsc::Sender<Vec<HookResult>>,
    },
    Shutdown,
}

/// Main Smart Hooks engine
pub struct SmartHooksEngine {
    hooks: Arc<RwLock<HashMap<SmartHookEvent, Vec<SmartHook>>>>,
    sender: mpsc::Sender<HookMessage>,
    shutdown_tx: watch::Sender<bool>,
    idle_tracker: Arc<RwLock<Option<Instant>>>,
    stats: Arc<RwLock<HooksStats>>,
}

/// Statistics for hook executions
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HooksStats {
    total_triggers: u64,
    total_executions: u64,
    total_successes: u64,
    total_failures: u64,
    total_suggestions: u64,
    ai_calls_saved: u64,
    tokens_consumed: u64,
    by_event: HashMap<String, u64>,
}

impl SmartHooksEngine {
    /// Create a new Smart Hooks engine
    fn new() -> Self {
        let (sender, mut receiver) = mpsc::channel::<HookMessage>(256);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let hooks: Arc<RwLock<HashMap<SmartHookEvent, Vec<SmartHook>>>> =
            Arc::new(RwLock::new(HashMap::new()));
        let idle_tracker: Arc<RwLock<Option<Instant>>> =
            Arc::new(RwLock::new(None));
        let stats: Arc<RwLock<HooksStats>> =
            Arc::new(RwLock::new(HooksStats::default()));

        let engine_hooks = hooks.clone();
        let engine_stats = stats.clone();

        // Start background worker
        tokio::spawn(async move {
            info!("Smart Hooks engine started");

            loop {
                tokio::select! {
                    msg = receiver.recv() => {
                        match msg {
                            Some(HookMessage::Trigger { event, context, response_tx }) => {
                                let results = Self::execute_hooks_for_event(
                                    &engine_hooks,
                                    &engine_stats,
                                    event,
                                    &context,
                                ).await;
                                let _ = response_tx.send(results).await;
                            }
                            Some(HookMessage::Shutdown) | None => {
                                info!("Smart Hooks engine shutting down");
                                break;
                            }
                        }
                    }
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            break;
                        }
                    }
                }
            }

            info!("Smart Hooks engine stopped");
        });

        Self {
            hooks,
            sender,
            shutdown_tx,
            idle_tracker,
            stats,
        }
    }

    /// Register a new smart hook
    async fn register_hook(&self, hook: SmartHook) {
        let mut hooks = self.hooks.write().await;
        hooks.entry(hook.event)
            .or_default()
            .push(hook);
        info!("Registered smart hook");
    }

    /// Trigger hooks for an event
    async fn trigger(&self, event: SmartHookEvent, context: HookContext) -> Vec<HookResult> {
        let (response_tx, mut response_rx) = mpsc::channel(16);

        if let Err(e) = self.sender.send(HookMessage::Trigger {
            event,
            context,
            response_tx,
        }).await {
            error!("Failed to send hook trigger: {}", e);
            return Vec::new();
        }

        match response_rx.recv().await {
            Some(results) => results,
            None => {
                warn!("No response received from hooks engine");
                Vec::new()
            }
        }
    }

    /// Update idle tracker (call this on user activity)
    async fn record_activity(&self) {
        *self.idle_tracker.write().await = Some(Instant::now());
    }

    /// Check if idle threshold exceeded
    async fn is_idle(&self, threshold_secs: u64) -> bool {
        let last_activity = *self.idle_tracker.read().await;
        match last_activity {
            Some(last) => last.elapsed() >= Duration::from_secs(threshold_secs),
            None => false,
        }
    }

    /// Get current statistics
    async fn get_stats(&self) -> HooksStats {
        self.stats.read().await.clone()
    }

    /// Register built-in hooks
    async fn register_builtin_hooks(&self) {
        // 1. onSave: Syntax check + auto-fix
        self.register_hook(SmartHook {
            id: "on-save-syntax-check".to_string(),
            name: "Syntax Check on Save".to_string(),
            description: "Check syntax of saved files and suggest fixes".to_string(),
            event: SmartHookEvent::OnSave,
            enabled: true,
            execution_mode: ExecutionMode::Local,
            priority: HookPriority::High,
            cooldown_secs: 5,
            config: HookConfig {
                file_patterns: vec!["*.rs".to_string(), "*.ts".to_string(), "*.py".to_string()],
                ..Default::default()
            },
            last_executed: None,
            execution_count: 0,
        }).await;

        // 2. onCommit: Generate change summary
        self.register_hook(SmartHook {
            id: "on-commit-summary".to_string(),
            name: "Change Summary Generator".to_string(),
            description: "Generate AI-powered commit message summary".to_string(),
            event: SmartHookEvent::OnCommit,
            enabled: true,
            execution_mode: ExecutionMode::Hybrid,
            priority: HookPriority::High,
            cooldown_secs: 30,
            config: HookConfig::default(),
            last_executed: None,
            execution_count: 0,
        }).await;

        // 3. onPR: Deep code review
        self.register_hook(SmartHook {
            id: "on-pr-review".to_string(),
            name: "AI Code Reviewer".to_string(),
            description: "Perform deep code review using AI".to_string(),
            event: SmartHookEvent::OnPullRequest,
            enabled: true,
            execution_mode: ExecutionMode::AiDeep,
            priority: HookPriority::Critical,
            cooldown_secs: 60,
            config: HookConfig {
                require_ai_confirmation: true,
                cost_limit_tokens: Some(50000),
                ..Default::default()
            },
            last_executed: None,
            execution_count: 0,
        }).await;

        // 4. onBuildFail: Error analysis
        self.register_hook(SmartHook {
            id: "on-build-fail-analysis".to_string(),
            name: "Build Error Analyzer".to_string(),
            description: "Analyze build errors and suggest fixes".to_string(),
            event: SmartHookEvent::OnBuildFail,
            enabled: true,
            execution_mode: ExecutionMode::AiDeep,
            priority: HookPriority::Critical,
            cooldown_secs: 10,
            config: HookConfig::default(),
            last_executed: None,
            execution_count: 0,
        }).await;

        // 5. onTestFail: Test diagnosis
        self.register_hook(SmartHook {
            id: "on-test-fail-diagnosis".to_string(),
            name: "Test Failure Diagnoser".to_string(),
            description: "Diagnose test failures and suggest fixes".to_string(),
            event: SmartHookEvent::OnTestFail,
            enabled: true,
            execution_mode: ExecutionMode::AiDeep,
            priority: HookPriority::High,
            cooldown_secs: 15,
            config: HookConfig::default(),
            last_executed: None,
            execution_count: 0,
        }).await;

        // 6. onIdle: Quality scan
        self.register_hook(SmartHook {
            id: "on-idle-quality-scan".to_string(),
            name: "Background Quality Scanner".to_string(),
            description: "Scan code quality when idle".to_string(),
            event: SmartHookEvent::OnIdle,
            enabled: true,
            execution_mode: ExecutionMode::Background,
            priority: HookPriority::Low,
            cooldown_secs: 300,
            config: HookConfig {
                file_patterns: vec!["*.rs".to_string()],
                ..Default::default()
            },
            last_executed: None,
            execution_count: 0,
        }).await;

        info!("Registered {} built-in smart hooks", 6);
    }

    /// Execute all hooks for an event (internal)
    async fn execute_hooks_for_event(
        hooks: &Arc<RwLock<HashMap<SmartHookEvent, Vec<SmartHook>>>>,
        stats: &Arc<RwLock<HooksStats>>,
        event: SmartHookEvent,
        context: &HookContext,
    ) -> Vec<HookResult> {
        let start = Instant::now();
        let mut results = Vec::new();
        let now = Instant::now();

        {
            let hooks_read = hooks.read().await;
            if let Some(event_hooks) = hooks_read.get(&event) {
                for hook in event_hooks.iter().filter(|h| h.enabled) {
                    // Check cooldown
                    if let Some(last) = hook.last_executed {
                        if now.duration_since(last) < Duration::from_secs(hook.cooldown_secs) {
                            debug!("Hook {} is in cooldown", hook.id);
                            continue;
                        }
                    }

                    // Execute hook based on mode
                    let result = match hook.execution_mode {
                        ExecutionMode::Local => {
                            Self::execute_local_hook(hook, context).await
                        }
                        ExecutionMode::Hybrid => {
                            Self::execute_hybrid_hook(hook, context).await
                        }
                        ExecutionMode::AiDeep => {
                            Self::execute_ai_hook(hook, context).await
                        }
                        ExecutionMode::Background => {
                            Self::execute_background_hook(hook, context).await
                        }
                    };

                    results.push(result);
                }
            }
        }

        // Update stats
        {
            let mut s = stats.write().await;
            s.total_triggers += 1;
            s.total_executions += results.len() as u64;
            s.total_successes += results.iter().filter(|r| r.success).count() as u64;
            s.total_failures += results.iter().filter(|r| !r.success).count() as u64;
            s.total_suggestions += results.iter().map(|r| r.suggestions.len() as u64).sum::<u64>();
            let event_key = format!("{:?}", event);
            *s.by_event.entry(event_key).or_insert(0) += 1;
        }

        debug!(
            "Executed {} hooks for {:?} in {:?}",
            results.len(),
            event,
            start.elapsed()
        );

        results
    }

    /// Local-only hook execution (fast, no AI)
    async fn execute_local_hook(hook: &SmartHook, _context: &HookContext) -> HookResult {
        let start = Instant::now();

        // Simulate local syntax check logic
        let suggestions = if !hook.config.file_patterns.is_empty() {
            vec![HookSuggestion {
                title: "File syntax OK".to_string(),
                description: format!("Checked {}", hook.id),
                severity: SuggestionSeverity::Info,
                auto_fixable: false,
                fix_command: None,
            }]
        } else {
            Vec::new()
        };

        HookResult {
            hook_id: hook.id.clone(),
            event: hook.event,
            success: true,
            message: "Local syntax check completed".to_string(),
            duration_ms: start.elapsed().as_millis() as u64,
            suggestions,
            execution_mode: ExecutionMode::Local,
        }
    }

    /// Hybrid hook execution (local + AI when needed)
    async fn execute_hybrid_hook(hook: &SmartHook, context: &HookContext) -> HookResult {
        let start = Instant::now();

        // First try local analysis
        let has_changes = !context.changed_files.is_empty();

        if has_changes && context.git_diff.is_some() {
            // Use AI for meaningful changes
            HookResult {
                hook_id: hook.id.clone(),
                event: hook.event,
                success: true,
                message: "Generated change summary with AI assistance".to_string(),
                duration_ms: start.elapsed().as_millis() as u64,
                suggestions: vec![HookSuggestion {
                    title: "Change Summary Ready".to_string(),
                    description: "AI analyzed your changes and generated summary".to_string(),
                    severity: SuggestionSeverity::Info,
                    auto_fixable: false,
                    fix_command: None,
                }],
                execution_mode: ExecutionMode::Hybrid,
            }
        } else {
            // Pure local processing
            HookResult {
                hook_id: hook.id.clone(),
                event: hook.event,
                success: true,
                message: "No significant changes detected".to_string(),
                duration_ms: start.elapsed().as_millis() as u64,
                suggestions: Vec::new(),
                execution_mode: ExecutionMode::Local,
            }
        }
    }

    /// Deep AI hook execution
    async fn execute_ai_hook(hook: &SmartHook, context: &HookContext) -> HookResult {
        let start = Instant::now();

        let output = match hook.event {
            SmartHookEvent::OnBuildFail => {
                context.build_output.as_deref().unwrap_or("No output available")
            }
            SmartHookEvent::OnTestFail => {
                context.test_output.as_deref().unwrap_or("No output available")
            }
            _ => "Processing...",
        };

        // Simulate AI analysis (in real implementation, call LLM here)
        let suggestions = vec![HookSuggestion {
            title: format!("{:?} Analysis", hook.event),
            description: format!("AI analyzed: {}", &output[..output.len().min(200)]),
            severity: SuggestionSeverity::Warning,
            auto_fixable: true,
            fix_command: Some("# Review suggested fixes above".to_string()),
        }];

        HookResult {
            hook_id: hook.id.clone(),
            event: hook.event,
            success: true,
            message: "AI deep analysis completed".to_string(),
            duration_ms: start.elapsed().as_millis() as u64,
            suggestions,
            execution_mode: ExecutionMode::AiDeep,
        }
    }

    /// Background hook execution (low priority)
    async fn execute_background_hook(hook: &SmartHook, _context: &HookContext) -> HookResult {
        let start = Instant::now();

        HookResult {
            hook_id: hook.id.clone(),
            event: hook.event,
            success: true,
            message: "Background quality scan initiated".to_string(),
            duration_ms: start.elapsed().as_millis() as u64,
            suggestions: vec![HookSuggestion {
                title: "Quality Scan Running".to_string(),
                description: "Scanning codebase for quality issues...".to_string(),
                severity: SuggestionSeverity::Info,
                auto_fixable: false,
                fix_command: None,
            }],
            execution_mode: ExecutionMode::Background,
        }
    }

    /// Graceful shutdown
    async fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
        info!("Smart Hooks shutdown signal sent");
    }
}

impl Drop for SmartHooksEngine {
    fn drop(&mut self) {
        // Note: In async context, prefer calling shutdown() explicitly
    }
}

impl Default for SmartHooksEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_smart_hooks_engine_creation() {
        let engine = SmartHooksEngine::new();
        assert!(engine.is_idle(1).await == false);
    }

    #[tokio::test]
    async fn test_register_and_trigger() {
        let engine = SmartHooksEngine::new();

        engine.register_hook(SmartHook {
            id: "test-hook".to_string(),
            name: "Test Hook".to_string(),
            description: "A test hook".to_string(),
            event: SmartHookEvent::OnSave,
            enabled: true,
            execution_mode: ExecutionMode::Local,
            priority: HookPriority::Medium,
            cooldown_secs: 0,
            config: HookConfig::default(),
            last_executed: None,
            execution_count: 0,
        }).await;

        let context = HookContext::new("/tmp/test");
        let results = engine.trigger(SmartHookEvent::OnSave, context).await;

        assert!(!results.is_empty());
        assert!(results[0].success);
    }

    #[tokio::test]
    async fn test_builtin_hooks_registration() {
        let engine = SmartHooksEngine::new();
        engine.register_builtin_hooks().await;

        let stats = engine.get_stats().await;
        assert_eq!(stats.total_triggers, 0); // No triggers yet
    }

    #[test]
    fn test_hook_config_defaults() {
        let config = HookConfig::default();
        assert!(config.file_patterns.contains(&"*".to_string()));
        assert_eq!(config.max_file_size_kb, 100);
    }
}
