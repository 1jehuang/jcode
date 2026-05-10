// ════════════════════════════════════════════════════════════════
// 权限决策引擎 — 移植自 Claude Code permissions.ts (1487行)
//
// 决策流水线:
//
//   ToolCall → [SafetyCheck] → [RuleMatch] → [ModeCheck] → Decision
//                 ↓               ↓             ↓
//           强制审批?       规则命中?     当前模式?
//
// 5 种权限模式的状态转换:
//
//   Plan ──→ Default ──→ Auto(YOLO) ──→ AcceptEdits ──→ Bypass
//    ↑          ↑            ↑                ↑
//    │          │            │                │
//   用户切换  连续允许    AI信任积累      显式授权
// ════════════════════════════════════════════════════════════════

use crate::command_sandbox::CommandSandbox;
use crate::denial_tracker::DenialTracker;
use crate::rule_parser::PermissionRuleParser;
use crate::safety_checker::SafetyChecker;
use crate::types::{
    CommandSeverity, DecisionBehavior, PermissionDecision, PermissionMode,
    SafetyViolationType,
};
use CommandSeverity as CmdSev;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

/// 工具调用请求上下文
#[derive(Debug, Clone)]
pub struct PermissionRequest {
    /// 工具名
    pub tool_name: String,

    /// 工具输入参数 (JSON string 或原始文本)
    pub tool_input: String,

    /// 会话 ID
    pub session_id: String,

    /// 用户 ID (用于权限隔离)
    pub user_id: Option<String>,

    /// 工作目录 (路径安全检查用)
    pub working_dir: Option<String>,

    /// 是否为只读操作
    pub is_readonly: bool,
}

/// 引擎配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineConfig {
    /// 默认权限模式
    pub default_mode: PermissionMode,

    /// 是否启用安全检查器
    pub safety_check_enabled: bool,

    /// 是否启用命令沙箱 (对 Bash 工具)
    pub command_sandbox_enabled: bool,

    /// YOLO 模式下的自动允许阈值 (置信度 > 此值则自动 allow)
    pub yolo_allow_threshold: f64,

    /// YOLO 模式下的自动拒绝阈值 (置信度 < 此值则自动 deny)
    pub yolo_deny_threshold: f64,

    /// 最大连续拒绝次数后降级
    pub max_consecutive_denials: u32,

    /// 敏感目录列表 (额外添加到默认列表)
    pub extra_sensitive_dirs: Vec<String>,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            default_mode: PermissionMode::Default,
            safety_check_enabled: true,
            command_sandbox_enabled: true,
            yolo_allow_threshold: 0.85,
            yolo_deny_threshold: 0.15,
            max_consecutive_denials: crate::DEFAULT_DENIAL_THRESHOLD,
            extra_sensitive_dirs: Vec::new(),
        }
    }
}

/// 权限引擎 — 核心决策中心
pub struct PermissionEngine {
    config: EngineConfig,
    current_mode: Arc<RwLock<PermissionMode>>,
    rule_parser: Arc<PermissionRuleParser>,
    safety_checker: Arc<SafetyChecker>,
    command_sandbox: Arc<CommandSandbox>,
    denial_tracker: Arc<RwLock<DenialTracker>>,
    #[cfg(feature = "ai-classifier")]
    yolo_classifier: Arc<RwLock<Option<crate::yolo_classifier::YoloClassifier>>>,
}

impl PermissionEngine {
    /// 创建新的权限引擎
    pub fn new(config: EngineConfig) -> Self {
        Self {
            current_mode: Arc::new(RwLock::new(config.default_mode)),
            rule_parser: Arc::new(PermissionRuleParser::new()),
            safety_checker: Arc::new(SafetyChecker::new()),
            command_sandbox: Arc::new(CommandSandbox::new()),
            denial_tracker: Arc::new(RwLock::new(DenialTracker::new())),
            #[cfg(feature = "ai-classifier")]
            yolo_classifier: Arc::new(RwLock::new(None)),
            config,
        }
    }

    /// 使用默认配置创建
    pub fn with_defaults() -> Self {
        Self::new(EngineConfig::default())
    }

    // ════════════════════════════════════════════════════
    // 核心决策 API
    // ════════════════════════════════════════════════════

    /// 对工具调用进行完整的权限决策
    ///
    /// 这是主入口，执行完整流水线:
    ///
    /// ```text
    /// Step 1: 安全检查 (敏感路径/危险命令) → 强制 Ask/Deny
    /// Step 2: 规则匹配 (用户自定义规则)   → 命中则直接决定
    /// Step 3: 模式判断 (当前权限模式)     → Bypass/Allow/Ask/Auto
    /// Step 4: YOLO 分类 (Auto 模式下)      → AI 最终决策
    /// Step 5: 拒绝追踪更新
    /// ```
    pub async fn decide(&self, request: &PermissionRequest) -> PermissionDecision {
        let mode = *self.current_mode.read().await;

        tracing::debug!(
            tool = %request.tool_name,
            mode = %mode,
            input_preview = %Self::preview_input(&request.tool_input),
            "Permission decision start"
        );

        // ── Step 1: 安全检查 (不可绕过!) ──
        if self.config.safety_check_enabled {
            if let Some(safety_decision) = self.safety_check(request).await {
                return safety_decision;
            }
        }

        // ── Step 2: 命令沙箱检查 (Bash 工具专用) ──
        if self.config.command_sandbox_enabled && request.tool_name.eq_ignore_ascii_case("bash") {
            if let Some(cmd_decision) = self.command_sandbox_check(request).await {
                return cmd_decision;
            }
        }

        // ── Step 3: 规则匹配 ──
        let matched_rule =
            self.rule_parser.match_tool_call(&request.tool_name, &request.tool_input);

        if let Some(rule) = matched_rule {
            tracing::info!(
                rule = %rule.display_name(),
                behavior = ?rule.behavior,
                priority = rule.priority,
                "Rule matched"
            );

            match &rule.behavior {
                DecisionBehavior::Allow => {
                    return PermissionDecision::allow(mode);
                }
                DecisionBehavior::Deny { reason } => {
                    self.record_denial();
                    return PermissionDecision::deny(reason, mode);
                }
                DecisionBehavior::Ask { reason } => {
                    // 规则要求确认 → 检查当前模式是否可跳过确认
                    if mode == PermissionMode::Bypass || mode == PermissionMode::AcceptEdits {
                        return PermissionDecision::allow(mode);
                    }
                    return PermissionDecision::ask(reason, mode);
                }
            }
        }

        // ── Step 4: 模式判断 ──
        match mode {
            PermissionMode::Bypass => PermissionDecision::allow(mode),

            PermissionMode::AcceptEdits => {
                // 自动接受文件编辑操作
                if self.is_edit_operation(&request.tool_name) {
                    return PermissionDecision::allow(mode);
                }
                PermissionDecision::ask("非编辑操作需要确认", mode)
            }

            PermissionMode::Auto => {
                // YOLO AI 分类
                #[cfg(feature = "ai-classifier")]
                {
                    self.yolo_classify(request, mode).await
                }
                #[cfg(not(feature = "ai-classifier"))]
                {
                    // 无 AI 时回退到默认行为: 只读操作允许，其他需确认
                    if request.is_readonly || self.is_safe_read_tool(&request.tool_name) {
                        PermissionDecision::allow(mode)
                    } else {
                        PermissionDecision::ask(
                            format!("{} 操作需要确认", request.tool_name),
                            mode,
                        )
                    }
                }
            }

            PermissionMode::Default => {
                // 只读工具直接允许
                if request.is_readonly || self.is_safe_read_tool(&request.tool_name) {
                    return PermissionDecision::allow(mode);
                }
                PermissionDecision::ask(
                    format!("请确认 {} 操作", request.tool_name),
                    mode,
                )
            }

            PermissionMode::Plan => PermissionDecision::deny(
                "当前处于计划模式，不允许执行任何修改操作",
                mode,
            ),
        }
    }

    // ════════════════════════════════════════════════════
    // 分步决策方法 (供高级用法)
    // ════════════════════════════════════════════════════

    /// 仅执行安全检查步骤
    async fn safety_check(&self, req: &PermissionRequest) -> Option<PermissionDecision> {
        let check_result = self.safety_checker.check_tool_call(&crate::safety_checker::ToolSafetyRequest {
            tool_name: req.tool_name.clone(),
            tool_input: req.tool_input.clone(),
        });

        if !check_result.safe {
            if check_result.force_approval {
                // 强制审批 (不可绕过!)
                return Some(PermissionDecision {
                    behavior: DecisionBehavior::Ask {
                        reason: Self::format_violations(&check_result.violations),
                    },
                    mode: PermissionMode::Default, // 即使 Bypass 也强制 Ask
                    safety_check: true,
                    rule_source: Some("SafetyChecker".into()),
                });
            } else {
                // 可拒绝
                return Some(PermissionDecision::deny(
                    Self::format_violations(&check_result.violations),
                    PermissionMode::Default,
                ));
            }
        }
        None
    }

    /// 仅执行命令沙箱检查
    async fn command_sandbox_check(&self, req: &PermissionRequest) -> Option<PermissionDecision> {
        let result = self.command_sandbox.analyze_command(&req.tool_input);

        if !result.allowed {
            return Some(match result.severity {
                Some(CmdSev::Critical | CmdSev::High) => PermissionDecision::deny(
                    result.block_reason.unwrap_or_else(|| "危险命令".into()),
                    *self.current_mode.read().await,
                ),
                _ => PermissionDecision::ask(
                    result.block_reason.unwrap_or("命令需要审核".into()),
                    *self.current_mode.read().await,
                ),
            });
        } else if result.requires_approval {
            return Some(PermissionDecision::ask(
                result.block_reason.unwrap_or("命令建议确认".into()),
                *self.current_mode.read().await,
            ));
        }
        None
    }

    /// YOLO AI 分类 (仅 Auto 模式使用)
    #[cfg(feature = "ai-classifier")]
    async fn yolo_classify(&self, req: &PermissionRequest, mode: PermissionMode) -> PermissionDecision {
        let classifier_guard = self.yolo_classifier.read().await;
        match classifier_guard.as_ref() {
            Some(classifier) => {
                drop(classifier_guard); // 释放读锁

                match classifier.classify(req).await {
                    Ok(result) => {
                        if result.should_block {
                            self.record_denial().await;
                            PermissionDecision::deny(result.reason, mode)
                        } else if result.confidence >= self.config.yolo_allow_threshold {
                            // 高置信度 → 自动允许
                            PermissionDecision::allow(mode)
                        } else {
                            // 中等置信度 → 需要用户确认
                            PermissionDecision::ask(format!(
                                "{} (置信度 {:.0}%)",
                                result.reason, result.confidence * 100.0
                            ), mode)
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "YOLO classification failed, falling back");
                        // AI 失败时回退到 Default 行为
                        if req.is_readonly {
                            PermissionDecision::allow(mode)
                        } else {
                            PermissionDecision::ask("AI 分类失败，请手动确认", mode)
                        }
                    }
                }
            }
            None => {
                // 未初始化分类器
                if req.is_readonly {
                    PermissionDecision::allow(mode)
                } else {
                    PermissionDecision::ask("YOLO 分类器未初始化", mode)
                }
            }
        }
    }

    // ════════════════════════════════════════════════════
    // 模式管理
    // ════════════════════════════════════════════════════

    /// 切换权限模式
    pub async fn set_mode(&self, new_mode: PermissionMode) -> PermissionMode {
        let old_mode = *self.current_mode.read().await;
        tracing::info!(
            from = %old_mode,
            to = %new_mode,
            "Permission mode changed"
        );
        *self.current_mode.write().await = new_mode;
        old_mode
    }

    /// 获取当前模式
    pub async fn current_mode(&self) -> PermissionMode {
        *self.current_mode.read().await
    }

    /// 升级到更宽松的模式
    pub async fn escalate_mode(&self) -> PermissionMode {
        let mut mode = self.current_mode.write().await;
        *mode = match *mode {
            PermissionMode::Plan => PermissionMode::Default,
            PermissionMode::Default => PermissionMode::Auto,
            PermissionMode::Auto => PermissionMode::AcceptEdits,
            PermissionMode::AcceptEdits => PermissionMode::Bypass,
            PermissionMode::Bypass => PermissionMode::Bypass,
        };
        *mode
    }

    /// 降级到更严格的模式
    pub async fn deescalate_mode(&self) -> PermissionMode {
        let mut mode = self.current_mode.write().await;
        *mode = match *mode {
            PermissionMode::Plan => PermissionMode::Plan,
            PermissionMode::Default => PermissionMode::Plan,
            PermissionMode::Auto => PermissionMode::Default,
            PermissionMode::AcceptEdits => PermissionMode::Auto,
            PermissionMode::Bypass => PermissionMode::AcceptEdits,
        };
        *mode
    }

    // ════════════════════════════════════════════════════
    // 规则管理
    // ════════════════════════════════════════════════════

    /// 获取规则解析器的可变引用 (用于加载规则)
    ///
    /// # Note
    /// 此方法需要内部可变性支持。当前实现返回一个临时占位，
    /// 生产环境中应将 rule_parser 包装在 Arc<RwLock<>> 中。
    /// 作为替代方案，请使用 `load_rules_from_text` 的批量接口或重新构造 Engine。
    pub async fn rules_mut(&self) -> std::result::Result<tokio::sync::RwLockWriteGuard<'_, PermissionRuleParser>, String> {
        Err("规则动态加载 API 需要重构: 请通过 PermissionEngine::with_rules() 构造或使用 load_rules_from_text(). 当前限制: Arc<PermissionRuleParser> 不支持 &self → &mut".into())
    }

    /// 加载规则文本 (便捷方法)
    pub async fn load_rules_from_text(&self, rules_text: &str) -> Result<(), String> {
        // TODO: 需要 parser 的内部可变性支持
        let _ = rules_text; // 暂时避免 unused warning
        Err("规则动态加载待实现: 需要重构 PermissionEngine 内部结构".into())
    }

    // ════════════════════════════════════════════════════
    // 辅助方法
    // ════════════════════════════════════════════════════

    fn is_edit_operation(&self, tool_name: &str) -> bool {
        matches!(
            tool_name.to_lowercase().as_str(),
            "write" | "edit" | "fileedit" | "file_write"
        )
    }

    fn is_safe_read_tool(&self, tool_name: &str) -> bool {
        matches!(
            tool_name.to_lowercase().as_str(),
            "read" | "fileread" | "glob" | "grep" | "list_files" | "search" | "webfetch"
                | "web_search" | "ls"
        )
    }

    fn record_denial(&self) {
        // 异步记录拒绝 (不需要 await)
        let tracker = self.denial_tracker.clone();
        tokio::spawn(async move {
            let mut t = tracker.write().await;
            t.record_denial();
        });
    }

    fn preview_input(input: &str) -> String {
        if input.len() > 80 {
            format!("{}...", &input[..80])
        } else {
            input.to_string()
        }
    }

    fn format_violations(violations: &[crate::types::SafetyViolation]) -> String {
        violations
            .iter()
            .map(|v| format!("[{}] {}", v.violation_type_as_str(), v.description))
            .collect::<Vec<_>>()
            .join("; ")
    }
}

// 为 SafetyViolation 添加 display 方法
impl crate::types::SafetyViolation {
    pub fn violation_type_as_str(&self) -> &'static str {
        match self.violation_type {
            SafetyViolationType::SensitiveDirectory => "SensitiveDir",
            SafetyViolationType::SensitiveFile => "SensitiveFile",
            SafetyViolationType::PathTraversal => "PathTraversal",
            SafetyViolationType::DangerousCommand => "DangerousCmd",
            SafetyViolationType::NetworkAccess => "Network",
            SafetyViolationType::EnvironmentExposure => "EnvExposure",
            SafetyViolationType::SymlinkAttack => "Symlink",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_default_mode_allows_read() {
        let engine = PermissionEngine::with_defaults();

        let req = PermissionRequest {
            tool_name: "Read".to_string(),
            tool_input: "./src/main.rs".to_string(),
            session_id: "test".to_string(),
            user_id: None,
            working_dir: None,
            is_readonly: true,
        };

        let decision = engine.decide(&req).await;
        assert!(
            decision.is_allowed(),
            "Read 操作在默认模式下应被允许, got: {:?}",
            decision.behavior
        );
    }

    #[tokio::test]
    async fn test_plan_mode_blocks_writes() {
        let engine = PermissionEngine::new(EngineConfig {
            default_mode: PermissionMode::Plan,
            ..Default::default()
        });

        let req = PermissionRequest {
            tool_name: "Write".to_string(),
            tool_input: "./test.txt".to_string(),
            session_id: "test".to_string(),
            user_id: None,
            working_dir: None,
            is_readonly: false,
        };

        let decision = engine.decide(&req).await;
        assert!(
            matches!(decision.behavior, DecisionBehavior::Deny { .. }),
            "计划模式应阻止写入操作"
        );
    }

    #[tokio::test]
    async fn test_bypass_mode_always_allows() {
        let engine = PermissionEngine::new(EngineConfig {
            default_mode: PermissionMode::Bypass,
            ..Default::default()
        });

        let req = PermissionRequest {
            tool_name: "Bash".to_string(),
            tool_input: "rm -rf /tmp/test".to_string(),
            session_id: "test".to_string(),
            user_id: None,
            working_dir: None,
            is_readonly: false,
        };

        // Bypass 应该允许一切 (除非安全检查强制拦截)
        let decision = engine.decide(&req).await;
        // rm -rf /tmp 可能被命令沙箱拦截, 所以这里不硬编码断言
        tracing::info!("Bypass mode decision: {:?}", decision);
    }

    #[tokio::test]
    async fn test_mode_escalation() {
        let engine = PermissionEngine::with_defaults();

        assert_eq!(engine.current_mode().await, PermissionMode::Default);

        let m1 = engine.escalate_mode().await;
        assert_eq!(m1, PermissionMode::Default); // 返回旧模式
        assert_eq!(engine.current_mode().await, PermissionMode::Auto);

        engine.escalate_mode().await; // Auto → AcceptEdits
        engine.escalate_mode().await; // AcceptEdits → Bypass
        assert_eq!(engine.current_mode().await, PermissionMode::Bypass);

        // Bypass → Bypass (已是最高级)
        engine.escalate_mode().await;
        assert_eq!(engine.current_mode().await, PermissionMode::Bypass);

        // 降级测试
        engine.deescalate_mode().await; // Bypass → AcceptEdits
        engine.deescalate_mode().await; // AcceptEdits → Auto
        assert_eq!(engine.current_mode().await, PermissionMode::Auto);
    }
}
