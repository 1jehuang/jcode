// jcode-sandbox
// ════════════════════════════════════════════════════════════════
// 安全沙箱与权限系统 - 移植自 Claude Code
//
// 核心能力:
//   1. 权限模式 —  default / plan / auto (YOLO) / acceptEdits / bypassPermissions
//   2. 规则系统 — ToolName(pattern) 语法, 支持 exact/prefix/wildcard 匹配
//   3. 安全检查器 (SafetyCheck) — .git/, .vscode/ 等敏感路径始终需要审批
//   4. 命令沙箱 — 危险命令检测、路径白名单、环境变量保护
//   5. YOLO 分类器 — AI 驱动的自动决策（可选功能）
//   6. 拒绝追踪 — 连续拒绝统计，自动降级为手动提示
//
// 对应 Claude Code 源码:
//   - src/types/permissions.ts (442行) — 类型定义
//   - src/utils/permissions/permissions.ts (1319行) — 核心流水线
//   - src/utils/permissions/permissionRuleParser.ts — 规则解析
//   - src/utils/permissions/safetyCheck.ts — 安全检查
//   - src/utils/permissions/yoloClassifier.ts — AI 分类器
// ════════════════════════════════════════════════════════════════

mod types;
mod permission_engine;
mod rule_parser;
mod safety_checker;
mod command_sandbox;
mod denial_tracker;
mod ssrf_guard;
#[cfg(feature = "ai-classifier")]
mod yolo_classifier;
mod auto_mode; // AutoMode 状态机

pub use types::*;
pub use permission_engine::{PermissionEngine, PermissionRequest, EngineConfig};
pub use rule_parser::PermissionRuleParser;
pub use safety_checker::{SafetyChecker, ToolSafetyRequest};
pub use command_sandbox::CommandSandbox;
pub use denial_tracker::DenialTracker;
pub use denial_tracker::DegradationAction;
pub use ssrf_guard::{SsrfGuard, SsrfGuardConfig, SsrfCheckResult};
pub use auto_mode::{AutoModeStateMachine, AutoModeEvent, AutoModeState};

/// 默认连续拒绝阈值 (触发降级)
pub const DEFAULT_DENIAL_THRESHOLD: u32 = 3;

/// 总拒绝上限 (超过后永久回退到手动模式)
pub const MAX_TOTAL_DENIALS: u32 = 20;

/// YOLO 拒绝后的自动降级次数
pub const YOLO_AUTO_FALLBACK_AFTER: u32 = 3;

/// Bash 工具默认超时 (秒)
pub const DEFAULT_BASH_TIMEOUT_SECS: u64 = 120;

/// 最大输出大小 (字节)
pub const MAX_COMMAND_OUTPUT_SIZE: usize = 10 * 1024 * 1024;  // 10MB

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_permission_mode_values() {
        assert_eq!(PermissionMode::default(), PermissionMode::Default);
        assert!(PermissionMode::Default < PermissionMode::Auto);  // Default < Auto in strictness
    }

    #[test]
    fn test_rule_parsing() {
        let parser = PermissionRuleParser::new();
        
        // 精确匹配
        let rule = parser.parse("Read(./src/main.ts)").unwrap();
        assert_eq!(rule.tool_name, "Read");
        
        // 前缀匹配
        let rule2 = parser.parse("Bash(git status:*)").unwrap();
        assert_eq!(rule2.match_type, RuleMatch::Prefix);
    }

    #[test]
    fn test_safety_check_sensitive_paths() {
        let checker = SafetyChecker::new();
        
        // .git 目录应触发安全检查
        let result = checker.check_path(".git/config");
        assert!(!result.is_safe());
        
        // 普通 src 文件应通过
        let result2 = checker.check_path("src/main.rs");
        assert!(result2.is_safe());
    }

    #[test]
    fn test_command_severity_classification() {
        let sandbox = CommandSandbox::new();
        
        // 危险命令
        assert_eq!(
            sandbox.classify_command("rm -rf /"),
            Some(CommandSeverity::Critical)
        );
        
        // 安全的只读命令
        assert_eq!(
            sandbox.classify_command("cat file.txt"),
            Some(CommandSeverity::Safe)
        );
    }
}
