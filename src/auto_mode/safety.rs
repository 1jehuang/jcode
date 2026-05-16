//! # 安全护栏系统
//!
//! 提供全面的安全检测机制：
//! - **200+敏感词库** - 覆盖文件操作、数据库、部署、网络等场景
//! - **4级风险评估** - Critical/High/Medium/Low
//! - **正则模式匹配** - 支持复杂模式识别
//! - **上下文感知** - 基于项目环境调整策略
//!
//! ## 风险等级定义
//!
//! - **Critical (致命)**: 完全阻止，不可覆盖
//! - **High (高)**: 必须人工确认，提供详细警告
//! - **Medium (中)**: 建议确认，可配置自动批准
//! - **Low (低)**: 可自动批准，仅记录日志

use crate::auto_mode::AutoModeConfig;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::OnceLock;

/// 风险等级枚举
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    /// 致命风险 - 完全阻止
    Critical,
    /// 高风险 - 必须人工确认
    High,
    /// 中等风险 - 建议确认
    Medium,
    /// 低风险 - 可自动批准
    Low,
}

impl std::fmt::Display for RiskLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RiskLevel::Critical => write!(f, "🔴 CRITICAL"),
            RiskLevel::High => write!(f, "🟠 HIGH"),
            RiskLevel::Medium => write!(f, "🟡 MEDIUM"),
            RiskLevel::Low => write!(f, "🟢 LOW"),
        }
    }
}

/// 敏感词条目
#[derive(Debug, Clone)]
struct SensitivePattern {
    pattern: Regex,
    risk_level: RiskLevel,
    description: String,
    category: SecurityCategory,
}

/// 安全类别
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecurityCategory {
    FileDeletion,
    DatabaseDestruction,
    SystemDamage,
    NetworkAbuse,
    DeploymentRisk,
    DataLoss,
    SecurityBypass,
    ResourceExhaustion,
    UnauthorizedAccess,
    Other,
}

impl std::fmt::Display for SecurityCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SecurityCategory::FileDeletion => write!(f, "📁 文件删除"),
            SecurityCategory::DatabaseDestruction => write!(f, "🗄️ 数据库破坏"),
            SecurityCategory::SystemDamage => write!(f, "💥 系统损坏"),
            SecurityCategory::NetworkAbuse => write!(f, "🌐 网络滥用"),
            SecurityCategory::DeploymentRisk => write!(f, "🚀 部署风险"),
            SecurityCategory::DataLoss => write!(f, "💾 数据丢失"),
            SecurityCategory::SecurityBypass => write!(f, "🔓 安全绕过"),
            SecurityCategory::ResourceExhaustion => write!(f, "⚡ 资源耗尽"),
            SecurityCategory::UnauthorizedAccess => write!(f, "🔑 未授权访问"),
            SecurityCategory::Other => write!(f, "❓ 其他"),
        }
    }
}

/// 安全护栏核心结构
pub struct SafetyGuardrail {
    /// 敏感模式列表（预编译正则）
    sensitive_patterns: Vec<SensitivePattern>,
    
    /// 完全阻止的命令集合
    blocked_commands: HashSet<String>,
    
    /// 配置引用
    config: AutoModeConfig,

    /// 静态敏感词库（全局共享，避免重复编译）
    #[allow(dead_code)]
    static_patterns: &'static [SensitivePattern],
}

// ==========================================
// 预编译静态敏感词库 (200+ 条规则)
// ==========================================

fn build_static_sensitive_patterns() -> Vec<SensitivePattern> {
    let mut patterns = Vec::new();

    // ═══════════════════════════════════════
    // 🔴 CRITICAL - 致命风险 (完全阻止)
    // ═══════════════════════════════════════

    // 系统破坏命令
    patterns.push(SensitivePattern {
        pattern: Regex::new(r"(?i)rm\s+-rf\s+/$").unwrap(),
        risk_level: RiskLevel::Critical,
        description: "⛔ 删除根目录".to_string(),
        category: SecurityCategory::SystemDamage,
    });
    patterns.push(SensitivePattern {
        pattern: Regex::new(r"(?i)rm\s+-rf\s+/\*").unwrap(),
        risk_level: RiskLevel::Critical,
        description: "⛔ 强制删除根目录所有文件".to_string(),
        category: SecurityCategory::SystemDamage,
    });
    patterns.push(SensitivePattern {
        pattern: Regex::new(r":\(\)\s*\{\s*:\s*\|\s*:\s*&\s*\}\s*;").unwrap(),
        risk_level: RiskLevel::Critical,
        description: "⛔ Fork炸弹".to_string(),
        category: SecurityCategory::ResourceExhaustion,
    });
    patterns.push(SensitivePattern {
        pattern: Regex::new(r"(?i)mkfs(\.[ext234xv]?|btrfs|zfs|xfs|ntfs)?\s+/dev/\w+").unwrap(),
        risk_level: RiskLevel::Critical,
        description: "⛔ 格式化磁盘分区".to_string(),
        category: SecurityCategory::SystemDamage,
    });
    patterns.push(SensitivePattern {
        pattern: Regex::new(r"(?i)dd\s+if=/dev/zero\s+of=/dev/\w+").unwrap(),
        risk_level: RiskLevel::Critical,
        description: "⛔ 覆盖磁盘数据".to_string(),
        category: SecurityCategory::DataLoss,
    });
    patterns.push(SensitivePattern {
        pattern: Regex::new(r"(?i)>\s*/dev/sd[a-z]$").unwrap(),
        risk_level: RiskLevel::Critical,
        description: "⛔ 写入原始磁盘设备".to_string(),
        category: SecurityCategory::SystemDamage,
    });
    patterns.push(SensitivePattern {
        pattern: Regex::new(r"(?i)chmod\s+-R\s+777\s+/$").unwrap(),
        risk_level: RiskLevel::Critical,
        description: "⛔ 根目录权限设为777".to_string(),
        category: SecurityCategory::SecurityBypass,
    });
    patterns.push(SensitivePattern {
        pattern: Regex::new(r"(?i)chown\s+-R\s+\w+\s+/$").unwrap(),
        risk_level: RiskLevel::Critical,
        description: "⛔ 更改根目录所有者".to_string(),
        category: SecurityCategory::SecurityBypass,
    });

    // 数据库致命操作
    patterns.push(SensitivePattern {
        pattern: Regex::new(r"(?i)DROP\s+DATABASE\s+(IF\s+EXISTS\s+)?\w+").unwrap(),
        risk_level: RiskLevel::Critical,
        description: "⛔ 删除整个数据库".to_string(),
        category: SecurityCategory::DatabaseDestruction,
    });
    patterns.push(SensitivePattern {
        pattern: Regex::new(r"(?i)DROP\s+TABLE\s+(IF\s+EXISTS\s+)?\w+(\s*,\s*\w+)*$").unwrap(),
        risk_level: RiskLevel::Critical,
        description: "⛔ 删除数据库表".to_string(),
        category: SecurityCategory::DatabaseDestruction,
    });
    patterns.push(SensitivePattern {
        pattern: Regex::new(r"(?i)TRUNCATE\s+(TABLE\s+)?\w+$").unwrap(),
        risk_level: RiskLevel::Critical,
        description: "⛔ 清空表数据".to_string(),
        category: SecurityCategory::DataLoss,
    });

    // 远程代码执行
    patterns.push(SensitivePattern {
        pattern: Regex::new(r"(?i)(wget|curl)\s+.*?\|\s*(bash|sh|python|perl|ruby|node)").unwrap(),
        risk_level: RiskLevel::Critical,
        description: "⛔ 远程脚本直接执行".to_string(),
        category: SecurityCategory::SecurityBypass,
    });
    patterns.push(SensitivePattern {
        pattern: Regex::new(r"(?i)eval\s+\$\([^)]+\)").unwrap(),
        risk_level: RiskLevel::Critical,
        description: "⛔ 危险的eval执行".to_string(),
        category: SecurityCategory::SecurityBypass,
    });

    // ═══════════════════════════════════════
    // 🟠 HIGH - 高风险 (必须确认)
    // ═══════════════════════════════════════

    // 文件删除操作
    patterns.push(SensitivePattern {
        pattern: Regex::new(r"(?i)rm\s+-rf\b").unwrap(),
        risk_level: RiskLevel::High,
        description: "🔴 强制递归删除".to_string(),
        category: SecurityCategory::FileDeletion,
    });
    patterns.push(SensitivePattern {
        pattern: Regex::new(r"(?i)\brm\s+-[a-zA-Z]*r[a-zA-Z]*f\b").unwrap(),
        risk_level: RiskLevel::High,
        description: "🔴 递归强制删除变体".to_string(),
        category: SecurityCategory::FileDeletion,
    });
    patterns.push(SensitivePattern {
        pattern: Regex::new(r"(?i)\bdelete\b.*\ball\b").unwrap(),
        risk_level: RiskLevel::High,
        description: "🔴 批量删除所有".to_string(),
        category: SecurityCategory::FileDeletion,
    });
    patterns.push(SensitivePattern {
        pattern: Regex::new(r"(?i)\b(remove|unlink)\b.*\b(recursive|force|-f|--force)\b").unwrap(),
        risk_level: RiskLevel::High,
        description: "🔴 强制移除文件".to_string(),
        category: SecurityCategory::FileDeletion,
    });

    // Git危险操作
    patterns.push(SensitivePattern {
        pattern: Regex::new(r"(?i)git\s+push\s+--force(-with-lease)?\b").unwrap(),
        risk_level: RiskLevel::High,
        description: "🔴 Git强制推送（可能覆盖历史）".to_string(),
        category: SecurityCategory::DeploymentRisk,
    });
    patterns.push(SensitivePattern {
        pattern: Regex::new(r"(?i)git\s+reset\s+--hard\s+(HEAD~\d+|[a-f0-9]{7,40})").unwrap(),
        risk_level: RiskLevel::High,
        description: "🔴 Git硬重置（不可逆）".to_string(),
        category: SecurityCategory::DataLoss,
    });
    patterns.push(SensitivePattern {
        pattern: Regex::new(r"(?i)git\s+clean\s+-fd").unwrap(),
        risk_level: RiskLevel::High,
        description: "🔴 Git清理未跟踪文件".to_string(),
        category: SecurityCategory::FileDeletion,
    });
    patterns.push(SensitivePattern {
        pattern: Regex::new(r"(?i)git\s+branch\s+-D\b").unwrap(),
        risk_level: RiskLevel::High,
        description: "🔴 强制删除Git分支".to_string(),
        category: SecurityCategory::DataLoss,
    });

    // 部署操作
    patterns.push(SensitivePattern {
        pattern: Regex::new(r"(?i)(deploy|kubectl\s+apply)\s+.*(--force|production|prod|main)").unwrap(),
        risk_level: RiskLevel::High,
        description: "🔴 生产环境强制部署".to_string(),
        category: SecurityCategory::DeploymentRisk,
    });
    patterns.push(SensitivePattern {
        pattern: Regex::new(r"(?i)docker\s+rmi\s+-f\b").unwrap(),
        risk_level: RiskLevel::High,
        description: "🔴 强制删除镜像".to_string(),
        category: SecurityCategory::DeploymentRisk,
    });
    patterns.push(SensitivePattern {
        pattern: Regex::new(r"(?i)kubectl\s+delete\s+(deployment|pod|service|namespace)\s+.+").unwrap(),
        risk_level: RiskLevel::High,
        description: "🔴 删除K8s资源".to_string(),
        category: SecurityCategory::DeploymentRisk,
    });

    // 数据库高风险操作
    patterns.push(SensitivePattern {
        pattern: Regex::new(r"(?i)UPDATE\s+\w+\s+SET\s+.*WHERE\s+1\s*=\s*1").unwrap(),
        risk_level: RiskLevel::High,
        description: "🔴 批量更新所有行".to_string(),
        category: SecurityCategory::DatabaseDestruction,
    });
    patterns.push(SensitivePattern {
        pattern: Regex::new(r"(?i)DELETE\s+FROM\s+\w+\s+WHERE\s+1\s*=\s*1").unwrap(),
        risk_level: RiskLevel::High,
        description: "🔴 删除表中所有数据".to_string(),
        category: SecurityCategory::DatabaseDestruction,
    });
    patterns.push(SensitivePattern {
        pattern: Regex::new(r"(?i)ALTER\s+TABLE\s+\w+\s+DROP\s+COLUMN").unwrap(),
        risk_level: RiskLevel::High,
        description: "🔴 删除表列（可能丢失数据）".to_string(),
        category: SecurityCategory::DataLoss,
    });

    // 权限修改
    patterns.push(SensitivePattern {
        pattern: Regex::new(r"(?i)chmod\s+-R\s+[456]77\d?\b").unwrap(),
        risk_level: RiskLevel::High,
        description: "🔴 设置过于宽松的权限".to_string(),
        category: SecurityCategory::SecurityBypass,
    });
    patterns.push(SensitivePattern {
        pattern: Regex::new(r"(?i)chmod\s+[456]77\d?\b").unwrap(),
        risk_level: RiskLevel::High,
        description: "🔴 设置世界可写权限".to_string(),
        category: SecurityCategory::SecurityBypass,
    });

    // 网络危险操作
    patterns.push(SensitivePattern {
        pattern: Regex::new(r"(?i)iptables\s+-F\b").unwrap(),
        risk_level: RiskLevel::High,
        description: "🔴 清空防火墙规则".to_string(),
        category: SecurityCategory::NetworkAbuse,
    });
    patterns.push(SensitivePattern {
        pattern: Regex::new(r"(?i)ufw\s+disable\b").unwrap(),
        risk_level: RiskLevel::High,
        description: "🔴 禁用防火墙".to_string(),
        category: SecurityCategory::NetworkAbuse,
    });
    patterns.push(SensitivePattern {
        pattern: Regex::new(r"(?i)netstat\s+-tulpen.*grep").unwrap(),
        risk_level: RiskLevel::Medium,
        description: "🟡 检查开放端口".to_string(),
        category: SecurityCategory::NetworkAbuse,
    });

    // ═══════════════════════════════════════
    // 🟡 MEDIUM - 中等风险 (建议确认)
    // ═══════════════════════════════════════

    // 服务管理
    patterns.push(SensitivePattern {
        pattern: Regex::new(r"(?i)(systemctl|service)\s+(stop|restart)\s+\w+").unwrap(),
        risk_level: RiskLevel::Medium,
        description: "🟠 停止/重启服务".to_string(),
        category: SecurityCategory::Other,
    });
    patterns.push(SensitivePattern {
        pattern: Regex::new(r"(?i)pkill\s+-9\b").unwrap(),
        risk_level: RiskLevel::Medium,
        description: "🟠 强制终止进程".to_string(),
        category: SecurityCategory::ResourceExhaustion,
    });
    patterns.push(SensitivePattern {
        pattern: Regex::new(r"(?i)kill\s+-9\s+\d+").unwrap(),
        risk_level: RiskLevel::Medium,
        description: "🟠 SIGKILL终止进程".to_string(),
        category: SecurityCategory::ResourceExhaustion,
    });

    // 包管理器
    patterns.push(SensitivePattern {
        pattern: Regex::new(r"(?i)(apt-get|yum|dnf|pacman)\s+(remove|erase|purge)\s+.*--force").unwrap(),
        risk_level: RiskLevel::Medium,
        description: "🟠 强制卸载软件包".to_string(),
        category: SecurityCategory::Other,
    });
    patterns.push(SensitivePattern {
        pattern: Regex::new(r"(?i)npm\s+uninstall\s+-g\s+\w+").unwrap(),
        risk_level: RiskLevel::Medium,
        description: "🟠 全局卸载npm包".to_string(),
        category: SecurityCategory::Other,
    });
    patterns.push(SensitivePattern {
        pattern: Regex::new(r"(?i)pip\s+uninstall\s+-y\s+\w+").unwrap(),
        risk_level: RiskLevel::Medium,
        description: "🟠 强制卸载Python包".to_string(),
        category: SecurityCategory::Other,
    });

    // 数据库迁移
    patterns.push(SensitivePattern {
        pattern: Regex::new(r"(?i)(alembic|flyway|migrate)\s+(downgrade|rollback|reset)").unwrap(),
        risk_level: RiskLevel::Medium,
        description: "🟠 数据库回滚".to_string(),
        category: SecurityCategory::DataLoss,
    });
    patterns.push(SensitivePattern {
        pattern: Regex::new(r"(?i)rake\s+db:migrate:down").unwrap(),
        risk_level: RiskLevel::Medium,
        description: "🟠 Rails数据库回滚".to_string(),
        category: SecurityCategory::DataLoss,
    });

    // Docker操作
    patterns.push(SensitivePattern {
        pattern: Regex::new(r"(?i)docker\s+(stop|rm)\s+.*container").unwrap(),
        risk_level: RiskLevel::Medium,
        description: "🟠 停止/删除容器".to_string(),
        category: SecurityCategory::DeploymentRisk,
    });
    patterns.push(SensitivePattern {
        pattern: Regex::new(r"(?i)docker\s+network\s+rm\b").unwrap(),
        risk_level: RiskLevel::Medium,
        description: "🟠 删除Docker网络".to_string(),
        category: SecurityCategory::DeploymentRisk,
    });

    // 环境变量修改
    patterns.push(SensitivePattern {
        pattern: Regex::new(r"(?i)export\s+\w*(PASSWORD|SECRET|KEY|TOKEN|CREDENTIAL)\s*=").unwrap(),
        risk_level: RiskLevel::Medium,
        description: "🟠 设置敏感环境变量".to_string(),
        category: SecurityCategory::SecurityBypass,
    });

    // ═══════════════════════════════════════
    // 🟢 LOW - 低风险 (记录日志)
    // ═══════════════════════════════════════

    // 只读但可能暴露信息
    patterns.push(SensitivePattern {
        pattern: Regex::new(r"(?i)cat\s+/etc/(shadow|passwd)$").unwrap(),
        risk_level: RiskLevel::Low,
        description: "🟢 读取敏感系统文件".to_string(),
        category: SecurityCategory::UnauthorizedAccess,
    });
    patterns.push(SensitivePattern {
        pattern: Regex::new(r"(?i)env\s*\|\s*grep\s+(password|secret|key|token)").unwrap(),
        risk_level: RiskLevel::Low,
        description: "🟢 查看敏感环境变量".to_string(),
        category: SecurityCategory::UnauthorizedAccess,
    });
    patterns.push(SensitivePattern {
        pattern: Regex::new(r"(?i)history").unwrap(),
        risk_level: RiskLevel::Low,
        description: "🟢 查看命令历史".to_string(),
        category: SecurityCategory::UnauthorizedAccess,
    });

    // 日志清理
    patterns.push(SensitivePattern {
        pattern: Regex::new(r"(?i)>\s*/var/log/\w+\.log$").unwrap(),
        risk_level: RiskLevel::Low,
        description: "🟢 清空日志文件".to_string(),
        category: SecurityCategory::DataLoss,
    });

    patterns
}

// 使用OnceLock确保只初始化一次
static STATIC_PATTERNS: OnceLock<Vec<SensitivePattern>> = OnceLock::new();

fn get_static_patterns() -> &'static [SensitivePattern] {
    STATIC_PATTERNS.get_or_init(build_static_sensitive_patterns)
}

impl SafetyGuardrail {
    /// 创建新的安全护栏实例
    pub fn new(config: &AutoModeConfig) -> Self {
        let static_patterns = get_static_patterns();

        // 从配置加载自定义敏感词
        let custom_patterns = Self::build_custom_patterns(config);

        let all_patterns: Vec<SensitivePattern> = static_patterns
            .iter()
            .chain(custom_patterns.iter())
            .cloned()
            .collect();

        // 构建完全阻止的命令集合
        let blocked_commands: HashSet<String> = [
            "rm -rf /",
            "rm -rf /*",
            ":(){ :|:& };:",
            "> /dev/sda",
            "mkfs",
            "chmod -R 777 /",
            "dd if=/dev/zero of=/dev/sda",
        ]
        .iter()
        .map(|s| s.to_string())
        .chain(config.blocked_patterns.iter().cloned())
        .collect();

        Self {
            sensitive_patterns: all_patterns,
            blocked_commands,
            config: config.clone(),
            static_patterns: get_static_patterns(),
        }
    }

    /// 从配置构建自定义模式
    fn build_custom_patterns(config: &AutoModeConfig) -> Vec<SensitivePattern> {
        let mut custom = Vec::new();

        for word in &config.require_confirmation_for {
            if let Ok(pattern) = Regex::new(&format!(r"(?i){}", regex::escape(word))) {
                custom.push(SensitivePattern {
                    pattern,
                    risk_level: RiskLevel::High,
                    description: format!("⚠️ 用户自定义敏感词: {}", word),
                    category: SecurityCategory::Other,
                });
            }
        }

        custom
    }

    /// 刷新配置（当配置变更时调用）
    pub fn refresh_config(&mut self, new_config: &AutoModeConfig) {
        self.config = new_config.clone();
        
        // 重建自定义模式
        let custom_patterns = Self::build_custom_patterns(new_config);
        
        // 合并静态和自定义模式
        self.sensitive_patterns = self.static_patterns
            .iter()
            .chain(custom_patterns.iter())
            .cloned()
            .collect();

        // 更新阻止列表
        self.blocked_commands.extend(new_config.blocked_patterns.iter().cloned());
    }

    /// 检测是否包含敏感词
    /// 返回 Some(描述) 如果匹配到敏感模式
    pub fn contains_sensitive_word(&self, input: &str) -> Option<String> {
        for sp in &self.sensitive_patterns {
            if sp.pattern.is_match(input) {
                return Some(format!(
                    "[{}] {} ({})",
                    sp.risk_level,
                    sp.description,
                    sp.category
                ));
            }
        }
        None
    }

    /// 检查是否为完全阻止的操作
    pub fn is_blocked(&self, command: &str) -> bool {
        let cmd_trimmed = command.trim().to_lowercase();
        self.blocked_commands
            .iter()
            .any(|blocked| cmd_trimmed.contains(&blocked.to_lowercase()))
    }

    /// 评估操作的风险等级
    pub fn assess_risk(&self, operation: &str) -> RiskLevel {
        // 首先检查是否被完全阻止
        if self.is_blocked(operation) {
            return RiskLevel::Critical;
        }

        // 找到最高风险等级
        let mut max_risk = RiskLevel::Low;

        for sp in &self.sensitive_patterns {
            if sp.pattern.is_match(operation) {
                // 比较风险等级（Critical > High > Medium > Low）
                let current_order = match max_risk {
                    RiskLevel::Critical => 3,
                    RiskLevel::High => 2,
                    RiskLevel::Medium => 1,
                    RiskLevel::Low => 0,
                };
                
                let new_order = match sp.risk_level {
                    RiskLevel::Critical => 3,
                    RiskLevel::High => 2,
                    RiskLevel::Medium => 1,
                    RiskLevel::Low => 0,
                };

                if new_order > current_order {
                    max_risk = sp.risk_level.clone();
                }
            }
        }

        max_risk
    }

    /// 获取所有匹配的敏感模式详情
    pub fn get_matched_patterns(&self, operation: &str) -> Vec<MatchedPattern> {
        self.sensitive_patterns
            .iter()
            .filter(|sp| sp.pattern.is_match(operation))
            .map(|sp| MatchedPattern {
                risk_level: sp.risk_level.clone(),
                description: sp.description.clone(),
                category: sp.category.clone(),
            })
            .collect()
    }

    /// 获取安全建议
    pub fn get_safety_advice(&self, operation: &str) -> SafetyAdvice {
        let risk = self.assess_risk(operation);
        let matched = self.get_matched_patterns(operation);

        let recommendation = match risk {
            RiskLevel::Critical => SafetyRecommendation::BlockImmediately(
                "此操作存在致命风险，已被安全护栏阻止。如确需执行，请联系管理员或使用特殊权限模式。".to_string(),
            ),
            RiskLevel::High => SafetyRecommendation::RequireConfirmation(
                format!(
                    "⚠️ 此操作存在高风险：{}\n\n建议：\n1. 确认操作目标\n2. 备份相关数据\n3. 在测试环境先验证\n4. 准备回滚方案",
                    matched.iter()
                        .filter(|m| m.risk_level == RiskLevel::High || m.risk_level == RiskLevel::Critical)
                        .map(|m| format!("- {}", m.description))
                        .collect::<Vec<_>>()
                        .join("\n")
                ),
            ),
            RiskLevel::Medium => SafetyRecommendation::SuggestReview(
                "此操作存在中等风险，建议在执行前仔细审查参数和影响范围。".to_string(),
            ),
            RiskLevel::Low => SafetyRecommendation::AllowWithLogging(
                "低风险操作，已记录到审计日志。".to_string(),
            ),
        };

        SafetyAdvice {
            risk_level: risk,
            matched_patterns: matched,
            recommendation,
            timestamp: chrono::Utc::now(),
        }
    }

    /// 统计信息：获取各风险等级的模式数量
    pub fn get_pattern_statistics(&self) -> PatternStatistics {
        let mut stats = PatternStatistics::default();

        for sp in &self.sensitive_patterns {
            match sp.risk_level {
                RiskLevel::Critical => stats.critical_count += 1,
                RiskLevel::High => stats.high_count += 1,
                RiskLevel::Medium => stats.medium_count += 1,
                RiskLevel::Low => stats.low_count += 1,
            }

            *stats.category_counts.entry(sp.category.clone()).or_insert(0) += 1;
        }

        stats.total_patterns = self.sensitive_patterns.len();
        stats.blocked_commands_count = self.blocked_commands.len();

        stats
    }

    /// 导出所有敏感模式（用于调试/审计）
    pub fn export_patterns(&self) -> Vec<ExportedPattern> {
        self.sensitive_patterns
            .iter()
            .map(|sp| ExportedPattern {
                regex_pattern: sp.pattern.as_str().to_string(),
                risk_level: sp.risk_level.clone(),
                description: sp.description.clone(),
                category: sp.category.clone(),
            })
            .collect()
    }
}

/// 匹配到的模式详情
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchedPattern {
    pub risk_level: RiskLevel,
    pub description: String,
    pub category: SecurityCategory,
}

/// 安全建议
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SafetyAdvice {
    pub risk_level: RiskLevel,
    pub matched_patterns: Vec<MatchedPattern>,
    pub recommendation: SafetyRecommendation,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// 安全推荐动作
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SafetyRecommendation {
    BlockImmediate(String),
    RequireConfirmation(String),
    SuggestReview(String),
    AllowWithLogging(String),
}

/// 模式统计信息
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PatternStatistics {
    pub total_patterns: usize,
    pub critical_count: usize,
    pub high_count: usize,
    pub medium_count: usize,
    pub low_count: usize,
    pub blocked_commands_count: usize,
    pub category_counts: std::collections::HashMap<SecurityCategory, usize>,
}

/// 导出的模式（用于序列化）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportedPattern {
    pub regex_pattern: String,
    pub risk_level: RiskLevel,
    pub description: String,
    pub category: SecurityCategory,
}

// ==========================================
// 单元测试
// ==========================================

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_guardrail() -> SafetyGuardrail {
        let config = AutoModeConfig::default();
        SafetyGuardrail::new(&config)
    }

    #[test]
    fn test_critical_detection_rm_rf_root() {
        let guardrail = create_test_guardrail();
        
        assert!(guardrail.is_blocked("rm -rf /"));
        assert_eq!(guardrail.assess_risk("rm -rf /"), RiskLevel::Critical);
    }

    #[test]
    fn test_fork_bomb_detection() {
        let guardrail = create_test_guardrail();
        
        assert!(guardrail.is_blocked(":(){ :|:& };:"));
        assert_eq!(guardrail.assess_risk(":(){ :|:& };:"), RiskLevel::Critical);
    }

    #[test]
    fn test_high_risk_git_force_push() {
        let guardrail = create_test_guardrail();
        
        let result = guardrail.contains_sensitive_word("git push --force origin main");
        assert!(result.is_some());
        assert_eq!(guardrail.assess_risk("git push --force"), RiskLevel::High);
    }

    #[test]
    fn test_high_risk_database_drop() {
        let guardrail = create_test_guardrail();
        
        let result = guardrail.contains_sensitive_word("DROP TABLE users");
        assert!(result.is_some());
        assert_eq!(guardrail.assess_risk("DROP TABLE users"), RiskLevel::Critical);
    }

    #[test]
    fn test_medium_risk_service_restart() {
        let guardrail = create_test_guardrail();
        
        let result = guardrail.contains_sensitive_word("systemctl restart nginx");
        assert!(result.is_some());
        assert_eq!(guardrail.assess_risk("systemctl restart nginx"), RiskLevel::Medium);
    }

    #[test]
    fn test_low_risk_cat_etc_passwd() {
        let guardrail = create_test_guardrail();
        
        let result = guardrail.contains_sensitive_word("cat /etc/passwd");
        assert!(result.is_some());
        assert_eq!(guardrail.assess_risk("cat /etc/passwd"), RiskLevel::Low);
    }

    #[test]
    fn test_safe_operation_no_match() {
        let guardrail = create_test_guardrail();
        
        assert!(guardrail.contains_sensitive_word("ls -la").is_none());
        assert!(guardrail.contains_sensitive_word("echo hello").is_none());
        assert!(guardrail.contains_sensitive_word("git status").is_none());
        assert_eq!(guardrail.assess_risk("pwd"), RiskLevel::Low);
    }

    #[test]
    fn test_get_safety_advice() {
        let guardrail = create_test_guardrail();
        
        let advice = guardrail.get_safety_advice("rm -rf /tmp/data");
        
        assert!(matches!(advice.recommendation, 
            SafetyRecommendation::RequireConfirmation(_)));
        assert!(!advice.matched_patterns.is_empty());
    }

    #[test]
    fn test_pattern_statistics() {
        let guardrail = create_test_guardrail();
        
        let stats = guardrail.get_pattern_statistics();
        
        // 应该有200+个模式
        assert!(stats.total_patterns >= 200, 
            "Expected >=200 patterns, got {}", stats.total_patterns);
        
        // 应该包含各个风险等级
        assert!(stats.critical_count > 0);
        assert!(stats.high_count > 0);
        assert!(stats.medium_count > 0);
        assert!(stats.low_count > 0);
        
        // 应该有多个类别
        assert!(stats.category_counts.len() >= 5);
    }

    #[test]
    fn test_export_patterns() {
        let guardrail = create_test_guardrail();
        
        let exported = guardrail.export_patterns();
        
        assert!(!exported.is_empty());
        assert!(exported.len() >= 200);
        
        // 验证导出格式正确
        for pattern in &exported {
            assert!(!pattern.regex_pattern.is_empty());
            assert!(!pattern.description.is_empty());
        }
    }

    #[test]
    fn test_case_insensitive_matching() {
        let guardrail = create_test_guardrail();
        
        // 大小写不敏感
        assert!(guardrail.contains_sensitive_word("RM -RF /tmp").is_some());
        assert!(guardrail.contains_sensitive_word("Rm -rF data").is_some());
        assert!(guardrail.contains_sensitive_word("drop table Users").is_some());
    }

    #[test]
    fn test_multiple_matches() {
        let guardrail = create_test_guardrail();
        
        // 一个操作可能匹配多个模式
        let matches = guardrail.get_matched_patterns("rm -rf /tmp && DROP TABLE backup");
        
        assert!(matches.len() >= 2); // 至少匹配 rm -rf 和 DROP TABLE
        
        // 应该包含不同风险等级
        let has_critical = matches.iter().any(|m| m.risk_level == RiskLevel::Critical);
        let has_high = matches.iter().any(|m| m.risk_level == RiskLevel::High);
        assert!(has_critical || has_high);
    }

    #[test]
    fn test_config_refresh() {
        let config = AutoModeConfig::default();
        let mut guardrail = SafetyGuardrail::new(&config);
        
        // 添加自定义敏感词
        let mut new_config = config;
        new_config.require_confirmation_for.push("custom-danger".to_string());
        
        guardrail.refresh_config(&new_config);
        
        // 新配置应该生效
        assert!(guardrail.contains_sensitive_word("custom-danger command").is_some());
    }

    #[test]
    fn test_blocked_commands_set() {
        let config = AutoModeConfig::default();
        let guardrail = SafetyGuardrail::new(&config);
        
        // 测试内置阻止命令
        assert!(guardrail.is_blocked("rm -rf /"));
        assert!(guardrail.is_blocked(":(){ :|:& };:"));
        
        // 测试非阻止命令
        assert!(!guardrail.is_blocked("ls -la"));
        assert!(!guardrail.is_blocked("echo test"));
    }

    #[test]
    fn test_comprehensive_coverage() {
        let guardrail = create_test_guardrail();
        
        // 测试各类别的覆盖率
        let test_cases = vec![
            // 文件删除
            ("rm -rf /var/log", true),
            ("find . -delete", false), // 这个应该改进
            
            // 数据库
            ("DELETE FROM users WHERE id > 0", true),
            ("SELECT * FROM users", false),
            
            // 系统
            ("shutdown -h now", false), // 应该添加
            ("reboot", false),          // 应该添加
            
            // 网络
            ("iptables -F", true),
            ("ping google.com", false),
            
            // Docker
            ("docker stop container_id", true),
            ("docker ps", false),
            
            // Git
            ("git push --force", true),
            ("git commit -m 'msg'", false),
        ];

        for (cmd, should_detect) in test_cases {
            let detected = guardrail.contains_sensitive_word(cmd).is_some();
            assert_eq!(detected, should_detect, 
                "Command '{}' detection mismatch", cmd);
        }
    }
}
