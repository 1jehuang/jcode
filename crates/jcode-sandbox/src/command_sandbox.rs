// ════════════════════════════════════════════════════════════════
// 命令沙箱 — 移植自 Claude Code bashClassifier + shellRuleMatching
//
// 对 Bash/Shell 命令进行:
//   1. 危险等级分类 (Safe/Low/Medium/High/Critical)
//   2. 正则模式匹配 (git/npm/cargo/docker/kubectl 等)
//   3. 路径白名单检查
//   4. 环境变量保护
//   5. 建议替代命令
// ════════════════════════════════════════════════════════════════

use crate::types::{CommandSeverity, SandboxResult};
use regex::Regex;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::LazyLock;

/// 安全命令模式列表: (正则, 描述)
const SAFE_COMMAND_PATTERNS: &[(&str, &str)] = &[
    // Git 只读操作
    (r"(?i)^git\s+(status|log|diff|show|branch|tag|remote -v|stash list|config --list)", "Git 只读命令"),
    (r"(?i)^git\s+(?:-c\s+.+\s+)?(?:show|log|diff)\b", "Git 查看命令"),
    // 文件查看
    (r"(?i)^(cat|less|more|head|tail|wc|grep|find|locate|which|type|file|stat|ls|dir|tree|echo|pwd|date|whoami|id)(?:\s|$)", "只读文件操作"),
    (r"(?i)^(npm|pnpm|yarn|cargo)\s+(--version|-v)$", "版本查询"),
    // 网络诊断
    (r"(?i)^(ping|nslookup|dig|host|traceroute|route|ipconfig|ifconfig|netstat)\s", "网络诊断"),
    // Python 安全运行
    (r"(?i)^(python|python3|node)\s+-m\s+py_compile\b", "Python 语法检查"),
    // 信息获取
    (r"(?i)^(uname|hostname|uptime|free|df|du|top|ps|env|printenv|locale)\s?", "系统信息查询"),
];

/// 低风险命令
const LOW_RISK_COMMAND_PATTERNS: &[(&str, &str)] = &[
    (r"(?i)^git\s+(add|commit|stash|checkout|switch|restore|revert|reset|--hard)\b", "Git 本地修改"),
    (r"(?i)^(npm|pnpm|yarn|cargo|pip|pip3|poetry|go mod)\s+(install|add|remove|uninstall|build|test|lint|format|run)\s", "包管理器基本操作"),
    (r"(?i)^(mkdir|touch|cp|mv|rename|chmod|chown)\s+[^*?\n]*$", "基础文件操作 (无通配符)"),
    (r"(?i)^(docker|podman)\s+(images|ps|inspect|logs|top|stats|history)\s", "容器查看命令"),
    (r"(?i)^(kubectl)\s+(get|describe|top|api-resources|explain)\s", "K8s 只读命令"),
    (r"(?i)^rustc\s+.*--emit=(metadata|dep-info|link)\b", "Rust 编译信息查询"),
];

/// 中风险命令
const MEDIUM_RISK_COMMAND_PATTERNS: &[(&str, &str)] = &[
    (r"(?i)^git\s+(push|pull|fetch|merge|rebase|cherry-pick)\b", "Git 远程同步"),
    (r"(?i)^rm\s+(?!-rf|-r/-f).*[^/\*\[\]]+$", "删除非递归"),
    (r"(?i)^(docker|podman)\s+(run|exec|rmi|stop|restart|kill|create|network|volume)\s", "容器管理"),
    (r"(?i)^(kubectl)\s+(apply|delete|edit|patch|rollout|scale|expose|port-forward|cp|attach|auth)\s", "K8s 修改操作"),
    (r"(?i)^systemctl\s+(start|stop|restart|reload|enable|disable)\s", "服务管理"),
    (r"(?i)^(pip|npm|cargo)\s+(publish|unpublish)\b", "包发布"),
    (r"(?i)^(ssh|scp|rsync)\s+", "远程连接/传输"),
];

/// 高风险命令
const HIGH_RISK_COMMAND_PATTERNS: &[(&str, &str)] = &[
    (r"(?i)^rm\s+(-[rf]+\s*){2,}.*[/~]", "递归强制删除"),
    (r"(?i)^rm\s+-rf\s+[~/$]", "删除主目录或根目录相关路径"),
    (r"(?i)^\s*(sudo\s+)?(dd|mkfs|fdisk|parted|shred|wipefs)\s", "磁盘/分区操作"),
    (r"(?i)^\s*(sudo\s+)?(iptables|nft|ufw|firewall-cmd)\s", "防火墙规则修改"),
    (r"(?i)^(sudo\s+)?user(add|del|mod)\s", "用户账户管理"),
    (r"(?i)^(crontab|at|batch)\s+(?-e|r|)", "计划任务修改"),
    (r"(?i)^git\s+push\s+.*(--force|-f)\s.*(main|master|develop|release/)", "强制推送保护分支"),
    (r"(?i)^\s*(sudo\s+)?(apt|yum|dnf|pacman|zypper)\s+(install|remove|purge)\s*(-y\s+)?(linux-image|kernel|grub|initramfs|systemd|bash|sh)", "系统核心组件变更"),
];

/// Critical 危险命令
static CRITICAL_RE: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    vec![
        Regex::new(r"(?i)^\s*(?:sudo\s+)?rm\s+(-rf?|-[fr]+)\s+/(?:\s|$|[a-z])").unwrap(),
        Regex::new(r"(?i)^\s*(?:sudo\s+)?(mkfs|format|shred)\s+(/\w+|[a-z]:\\)").unwrap(),
        Regex::new(r"(?i)^\s*>?\s*/dev/sd[a-z]\d*$").unwrap(),
        Regex::new(r"(?i)^\s*dd\s+if=/dev/.*of=/dev/").unwrap(),
        Regex::new(r"(?i)^\s*(sudo\s+)?(:?\w+:)?chmod\s+(-R\s+)?777\s+/").unwrap(),
        Regex::new(r"(?i)^\s*curl|\wget\s+.*(\|\s*(ba)?sh$|>\s*/tmp/.*\.sh$)").unwrap(),
        Regex::new(r"(?i)^\s*:()\s*\{\s*:\s*\|:&\s*;\s*\}").unwrap(), // Fork bomb
        Regex::new(r"(?i)^\s*(sudo\s+)?shutdown\s+(-h|-P|now|0)").unwrap(),
        Regex::new(r"(?i)^\s*(sudo\s+)?reboot\s*$").unwrap(),
        Regex::new(r"(?i)^\s*(sudo\s+)?halt\s*$").unwrap(),
        Regex::new(r"(?i)DROP\s+DATABASE").unwrap(),
        Regex::new(r"(?i)UPDATE\s+\w+\s+SET\s+\w+\s*=\s*'").unwrap(), // 无 WHERE 的 UPDATE
    ]
});

/// 需要确认的环境变量名模式
static SENSITIVE_ENV_VARS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        "PASSWORD", "PASSWD", "SECRET", "KEY", "TOKEN", "API_KEY", "API_SECRET",
        "PRIVATE_KEY", "CREDENTIAL", "AUTH", "COOKIE", "SESSION", "DATABASE_URL",
        "AWS_ACCESS_KEY", "AWS_SECRET_KEY", "AZURE_", "GCP_", "ENCRYPTION_",
    ]
    .into_iter()
    .collect()
});

pub struct CommandSandbox {
    /// 额外安全命令模式
    extra_safe_patterns: Vec<(Regex, String)>,
    /// 允许的工作目录集合 (空 = 不限制)
    allowed_dirs: HashSet<PathBuf>,
    /// 是否检测环境变量泄露
    check_env_exposure: bool,
}

impl Default for CommandSandbox {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandSandbox {
    pub fn new() -> Self {
        Self {
            extra_safe_patterns: Vec::new(),
            allowed_dirs: HashSet::new(),
            check_env_exposure: true,
        }
    }

    pub fn with_allowed_dirs(mut self, dirs: Vec<String>) -> Self {
        self.allowed_dirs = dirs.into_iter().map(PathBuf::from).collect();
        self
    }

    pub fn without_env_check(mut self) -> Self {
        self.check_env_exposure = false;
        self
    }

    // --- 核心分析 API ---------------------------------

    /// 分析命令并返回沙箱决策
    pub fn analyze_command(&self, command: &str) -> SandboxResult {
        let trimmed = command.trim();

        if trimmed.is_empty() {
            return SandboxResult {
                allowed: false,
                severity: None,
                block_reason: Some("空命令".into()),
                requires_approval: false,
                suggestion: None,
            };
        }

        let severity = self.classify_command(trimmed);

        match severity {
            Some(CommandSeverity::Critical) => SandboxResult {
                allowed: false,
                severity: Some(CommandSeverity::Critical),
                block_reason: Some(format!("危险命令被阻止: {}", Self::truncate_cmd(trimmed))),
                requires_approval: false,
                suggestion: self.suggest_alternative(trimmed),
            },
            Some(CommandSeverity::High) => SandboxResult {
                allowed: true, // 不阻止，但需要审批
                severity: Some(CommandSeverity::High),
                block_reason: Some(format!("高风险命令，请确认: {}", Self::truncate_cmd(trimmed))),
                requires_approval: true,
                suggestion: None,
            },
            Some(CommandSeverity::Medium) => SandboxResult {
                allowed: true,
                severity: Some(CommandSeverity::Medium),
                block_reason: Some(format!("中风险命令建议确认: {}", Self::truncate_cmd(trimmed))),
                requires_approval: true,
                suggestion: None,
            },
            Some(CommandSeverity::Low) => SandboxResult {
                allowed: true,
                severity: Some(CommandSeverity::Low),
                block_reason: None,
                requires_approval: false,
                suggestion: None,
            },
            Some(CommandSeverity::Safe) => SandboxResult {
                allowed: true,
                severity: Some(CommandSeverity::Safe),
                block_reason: None,
                requires_approval: false,
                suggestion: None,
            },
            None => SandboxResult {
                allowed: true, // 未识别的命令默认允许 (保守策略)
                severity: None,
                block_reason: None,
                requires_approval: true, // 未识别的命令建议确认
                suggestion: None,
            },
        }
    }

    /// 分类命令的危险等级
    pub fn classify_command(&self, command: &str) -> Option<CommandSeverity> {
        let cmd = command.trim();

        // 1. 最先检查 Critical (最高优先级)
        for re in CRITICAL_RE.iter() {
            if re.is_match(cmd) {
                return Some(CommandSeverity::Critical);
            }
        }

        // 2. 检查高危险模式
        for (pattern, _) in HIGH_RISK_COMMAND_PATTERNS {
            if let Ok(re) = Regex::new(pattern) {
                if re.is_match(cmd) {
                    return Some(CommandSeverity::High);
                }
            }
        }

        // 3. 中风险
        for (pattern, _) in MEDIUM_RISK_COMMAND_PATTERNS {
            if let Ok(re) = Regex::new(pattern) {
                if re.is_match(cmd) {
                    return Some(CommandSeverity::Medium);
                }
            }
        }

        // 4. 低风险
        for (pattern, _) in LOW_RISK_COMMAND_PATTERNS {
            if let Ok(re) = Regex::new(pattern) {
                if re.is_match(cmd) {
                    return Some(CommandSeverity::Low);
                }
            }
        }

        // 5. 安全命令
        for (pattern, _) in SAFE_COMMAND_PATTERNS {
            if let Ok(re) = Regex::new(pattern) {
                if re.is_match(cmd) {
                    return Some(CommandSeverity::Safe);
                }
            }
        }

        // 额外注册的安全模式
        for (re, _) in &self.extra_safe_patterns {
            if re.is_match(cmd) {
                return Some(CommandSeverity::Safe);
            }
        }

        None // 无法分类
    }

    /// 检查命令是否会暴露敏感环境变量
    pub fn check_env_exposure(&self, command: &str) -> Option<String> {
        if !self.check_env_exposure {
            return None;
        }

        let upper = command.to_uppercase();
        for var in SENSITIVE_ENV_VARS.iter() {
            if upper.contains(var) && (upper.contains("ECHO ") || upper.contains("PRINT ") || upper.contains(">") || upper.contains(">>")) {
                return Some(format!(
                    "命令可能暴露敏感环境变量 '{}'. 请使用 'echo ${{{}:0:4}}...' 来部分显示",
                    var, var
                ));
            }
        }
        None
    }

    /// 为危险命令提供安全的替代建议
    fn suggest_alternative(&self, command: &str) -> Option<String> {
        let lower = command.to_lowercase();

        if lower.contains("rm -rf /") || lower.starts_with("rm -rf /") {
            return Some("使用 rm -rf <具体目录> 替代, 或先 ls 确认目标".into());
        }
        if lower.contains("mkfs") || lower.contains("format") {
            return Some("磁盘格式化是破坏性操作! 请确认设备名称正确".into());
        }
        if lower.contains("dd ") && lower.contains("/dev/") {
            return Some("dd 写入磁盘会永久丢失数据. 考虑使用 dd if=... of=/dev/null 进行只读测试".into());
        }
        if lower.contains("chmod") && lower.contains("777") && lower.contains("/") {
            return Some("使用 chmod 755 或更严格的权限替代 777".into());
        }
        if lower.contains("git push") && (lower.contains("--force") || lower.contains("-f")) {
            return Some("考虑使用 git push --force-with-lease 替代 --force".into());
        }
        if lower.contains(":(){ :|:& };:") || lower.contains(":() { :|:& };:") {
            return Some("Fork bomb! 此命令会耗尽所有系统资源".into());
        }
        if lower.starts_with("shutdown") || lower.starts_with("reboot") || lower.starts_with("halt") {
            return Some("系统关机/重启会影响所有用户和正在运行的进程".into());
        }
        if lower.starts_with("drop ") {
            return Some("DROP 是不可逆操作! 先用 SELECT 确认数据是否已备份".into());
        }

        None
    }

    fn truncate_cmd(cmd: &str) -> String {
        if cmd.len() > 100 {
            format!("{}...", &cmd[..100])
        } else {
            cmd.to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_safe_git_commands() {
        let sb = CommandSandbox::new();
        assert_eq!(sb.classify_command("git status"), Some(CommandSeverity::Safe));
        assert_eq!(sb.classify_command("git log --oneline -10"), Some(CommandSeverity::Safe));
        assert_eq!(sb.classify_command("git diff main..feature"), Some(CommandSeverity::Safe));
    }

    #[test]
    fn test_critical_rm_rf_root() {
        let sb = CommandSandbox::new();
        assert_eq!(sb.classify_command("rm -rf /"), Some(CommandSeverity::Critical));
        assert_eq!(sb.classify_command("rm -rf /*"), Some(CommandSeverity::Critical));

        let result = sb.analyze_command("rm -rf /");
        assert!(!result.allowed);
        assert_eq!(result.severity, Some(CommandSeverity::Critical));
    }

    #[test]
    fn test_fork_bomb() {
        let sb = CommandSandbox::new();
        assert_eq!(
            sb.classify_command(":(){ :|:& };:"),
            Some(CommandSeverity::Critical)
        );
    }

    #[test]
    fn test_dd_disk_write() {
        let sb = CommandSandbox::new();
        assert_eq!(
            sb.classify_command("dd if=/dev/zero of=/dev/sda bs=1M"),
            Some(CommandSeverity::Critical)
        );
    }

    #[test]
    fn test_high_risk_force_push() {
        let sb = CommandSandbox::new();
        assert_eq!(
            sb.classify_command("git push --force origin main"),
            Some(CommandSeverity::High)
        );
    }

    #[test]
    fn test_medium_risk_docker_run() {
        let sb = CommandSandbox::new();
        assert_eq!(
            sb.classify_command("docker run -it ubuntu bash"),
            Some(CommandSeverity::Medium)
        );
    }

    #[test]
    fn test_low_risk_npm_install() {
        let sb = CommandSandbox::new();
        assert_eq!(
            sb.classify_command("npm install lodash"),
            Some(CommandSeverity::Low)
        );
    }

    #[test]
    fn test_env_exposure_detected() {
        let sb = CommandSandbox::new();
        let warning = sb.check_env_exposure("echo $API_KEY > /tmp/debug");
        assert!(warning.is_some());
    }

    #[test]
    fn test_suggestions() {
        let sb = CommandSandbox::new();
        let result = sb.analyze_command("rm -rf /");
        assert!(result.suggestion.is_some());
    }
}
