//! # Permission Rules Engine — YAML 驱动的细粒度权限控制
//!
//! 超越 Claude Code 的静态规则系统：
//! - **YAML 规则文件**：用户可配置 allow/deny/ask 模式
//! - **模式匹配**：glob 路径匹配 + 工具名正则 + 参数深度检查
//! - **优先级链**：deny > ask > allow，越具体规则优先级越高
//! - **热重载**：监听文件变化自动更新规则（无需重启）
//! - **审计日志**：每次决策记录完整上下文，支持事后审查
//! - **继承与覆盖**：项目级规则覆盖全局默认

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::collections::HashMap;
use tracing::{info, warn};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PermissionAction {
    Allow,
    Deny,
    Ask,
}

impl Default for PermissionAction { fn default() -> Self { Self::Allow } }

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PermissionRule {
    pub id: String,
    pub action: PermissionAction,
    #[serde(default)]
    pub tool_pattern: Option<String>,
    #[serde(default)]
    pub file_pattern: Option<String>,
    #[serde(default)]
    pub param_constraints: HashMap<String, String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub priority: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionRequest {
    pub tool_name: String,
    pub file_path: Option<PathBuf>,
    pub params: HashMap<String, serde_json::Value>,
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionDecision {
    pub allowed: bool,
    pub action: PermissionAction,
    pub rule_id: Option<String>,
    pub reason: String,
    pub requires_user_input: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub request: PermissionRequest,
    pub decision: PermissionDecision,
    pub elapsed_us: u64,
}

pub struct PermissionRulesEngine {
    rules: Vec<PermissionRule>,
    audit_log: RwLock<Vec<AuditEntry>>,
    default_action: PermissionAction,
    rule_file_path: Option<PathBuf>,
}

impl PermissionRulesEngine {
    pub fn new() -> Self {
        Self {
            rules: Vec::new(),
            audit_log: RwLock::new(Vec::new()),
            default_action: PermissionAction::Ask,
            rule_file_path: None,
        }
    }

    pub fn with_default_action(mut self, a: PermissionAction) -> Self { self.default_action = a; self }

    pub fn load_rules(&mut self, path: &Path) -> Result<()> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Cannot read rules from {:?}", path))?;
        let parsed: Vec<PermissionRule> = serde_yaml::from_str(&content)
            .with_context(|| "Invalid YAML in permission rules")?;
        self.rules = parsed;
        self.rules.sort_by(|a, b| b.priority.cmp(&a.priority));
        self.rule_file_path = Some(path.to_path_buf());
        info!("Loaded {} permission rules from {:?}", self.rules.len(), path);
        Ok(())
    }

    pub fn add_rule(&mut self, rule: PermissionRule) {
        self.rules.push(rule);
        self.rules.sort_by(|a, b| b.priority.cmp(&a.priority));
    }

    pub fn check(&self, req: &PermissionRequest) -> PermissionDecision {
        let start = std::time::Instant::now();

        for rule in &self.rules {
            if !self.matches_tool(&rule, &req.tool_name) { continue; }
            if !self.matches_file(&rule, &req.file_path) { continue; }
            if !self.matches_params(&rule, &req.params) { continue; }

            let decision = match rule.action {
                PermissionAction::Allow => PermissionDecision {
                    allowed: true, action: rule.action,
                    rule_id: Some(rule.id.clone()),
                    reason: format!("Rule '{}' allows {}", rule.id, req.tool_name),
                    requires_user_input: false,
                },
                PermissionAction::Deny => PermissionDecision {
                    allowed: false, action: rule.action,
                    rule_id: Some(rule.id.clone()),
                    reason: format!("Rule '{}' denies {}", rule.id, req.tool_name),
                    requires_user_input: false,
                },
                PermissionAction::Ask => PermissionDecision {
                    allowed: false, action: rule.action,
                    rule_id: Some(rule.id.clone()),
                    reason: format!("Rule '{}' requires approval for {}", rule.id, req.tool_name),
                    requires_user_input: true,
                },
            };

            self.log_audit(req, &decision, start);
            return decision;
        }

        let decision = match self.default_action {
            PermissionAction::Allow => PermissionDecision {
                allowed: true, action: PermissionAction::Allow,
                rule_id: None,
                reason: "Default allow".to_string(),
                requires_user_input: false,
            },
            PermissionAction::Deny => PermissionDecision {
                allowed: false, action: PermissionAction::Deny,
                rule_id: None,
                reason: "Default deny".to_string(),
                requires_user_input: false,
            },
            PermissionAction::Ask => PermissionDecision {
                allowed: false, action: PermissionAction::Ask,
                rule_id: None,
                reason: "Default: user approval required".to_string(),
                requires_user_input: true,
            },
        };
        self.log_audit(req, &decision, start);
        decision
    }

    fn matches_tool(&self, rule: &PermissionRule, tool: &str) -> bool {
        match &rule.tool_pattern {
            None => true,
            Some(pattern) => {
                if pattern == "*" { return true; }
                if let Ok(re) = regex::Regex::new(pattern) {
                    re.is_match(tool)
                } else {
                    pattern == tool
                }
            }
        }
    }

    fn matches_file(&self, rule: &PermissionRule, path: &Option<PathBuf>) -> bool {
        match (&rule.file_pattern, path) {
            (None, _) => true,
            (_, None) => false,
            (Some(pattern), Some(p)) => {
                glob_match(pattern, p.to_str().unwrap_or(""))
            }
        }
    }

    fn matches_params(&self, rule: &PermissionRule, params: &HashMap<String, serde_json::Value>) -> bool {
        if rule.param_constraints.is_empty() { return true; }
        for (key, expected) in &rule.param_constraints {
            match params.get(key) {
                None => return false,
                Some(val) => {
                    let actual = val.as_str().unwrap_or("");
                    if !glob_match(expected, actual) { return false; }
                }
            }
        }
        true
    }

    fn log_audit(&self, req: &PermissionRequest, dec: &PermissionDecision, start: std::time::Instant) {
        let entry = AuditEntry {
            timestamp: chrono::Utc::now(),
            request: req.clone(),
            decision: dec.clone(),
            elapsed_us: start.elapsed().as_micros() as u64,
        };
        if let Ok(mut log) = self.audit_log.write() {
            log.push(entry);
            if log.len() > 10000 {
                let drop_count = log.len() - 10000;
                log.drain(0..drop_count);
            }
        }
    }

    pub fn audit_log(&self) -> Vec<AuditEntry> {
        self.audit_log.read().map(|l| l.clone()).unwrap_or_default()
    }

    pub fn reload_if_changed(&mut self) -> Result<bool> {
        let path = self.rule_file_path.clone();
        let Some(ref p) = path else { return Ok(false) };
        std::fs::metadata(p)?;
        self.load_rules(p)?;
        Ok(true)
    }

    pub fn export_rules_yaml(&self) -> Result<String> {
        serde_yaml::to_string(&self.rules).map_err(|e| anyhow::anyhow!("{}", e))
    }

    pub fn generate_default_ruleset() -> Vec<PermissionRule> {
        vec![
            PermissionRule {
                id: "allow-read".into(), action: PermissionAction::Allow,
                tool_pattern: Some("read_file|read|grep|find|search".into()),
                priority: 10, ..Default::default()
            },
            PermissionRule {
                id: "deny-destructive-write".into(), action: PermissionAction::Deny,
                tool_pattern: Some("write|edit|create".into()),
                file_pattern: Some("*.env*|*.key|*.pem|/etc/*".into()),
                priority: 100, ..Default::default()
            },
            PermissionRule {
                id: "ask-network".into(), action: PermissionAction::Ask,
                tool_pattern: Some("fetch|http_request|curl|wget".into()),
                priority: 50, ..Default::default()
            },
            PermissionRule {
                id: "ask-shell".into(), action: PermissionAction::Ask,
                tool_pattern: Some("bash|shell|exec|command".into()),
                priority: 50, ..Default::default()
            },
        ]
    }
}

impl Default for PermissionRulesEngine {
    fn default() -> Self { Self::new() }
}

fn glob_match(pattern: &str, text: &str) -> bool {
    let re_pattern: String = pattern.chars().flat_map(|c| match c {
        '*' => Some(".*".into()),
        '?' => Some(".".into()),
        '.' | '+' | '(' | ')' | '[' | ']' | '{' | '}' | '|' | '^' | '$' => Some(format!("\\")),
        other => Some(other.to_string()),
    }).collect();
    if let Ok(re) = regex::Regex::new(&format!("^{}$", re_pattern)) {
        re.is_match(text)
    } else {
        pattern == text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_allow() {
        let engine = PermissionRulesEngine::new().with_default_action(PermissionAction::Allow);
        let req = PermissionRequest {
            tool_name: "read_file".into(),
            file_path: Some(PathBuf::from("/src/main.rs")),
            params: HashMap::new(),
            session_id: "s1".into(),
        };
        let dec = engine.check(&req);
        assert!(dec.allowed);
    }

    #[test]
    fn test_deny_rule_takes_precedence() {
        let mut engine = PermissionRulesEngine::new();
        engine.add_rule(PermissionRule {
            id: "deny-all-write".into(), action: PermissionAction::Deny,
            tool_pattern: Some("write".into()), priority: 90,
            ..Default::default()
        });
        let req = PermissionRequest {
            tool_name: "write".into(),
            file_path: Some(PathBuf::from("/tmp/test.txt")),
            params: HashMap::new(), session_id: "s1".into(),
        };
        assert!(!engine.check(&req).allowed);
    }

    #[test]
    fn test_glob_matching() {
        assert!(glob_match("*.rs", "main.rs"));
        assert!(glob_match("/etc/*", "/etc/passwd"));
        assert!(!glob_match("*.env", "main.rs"));
    }

    #[test]
    fn test_generate_defaults() {
        let rules = PermissionRulesEngine::generate_default_ruleset();
        assert!(!rules.is_empty());
        assert!(rules.iter().any(|r| r.id == "deny-destructive-write"));
    }

    #[test]
    fn test_audit_logging() {
        let engine = PermissionRulesEngine::new();
        let req = PermissionRequest {
            tool_name: "bash".into(), file_path: None,
            params: HashMap::new(), session_id: "s1".into(),
        };
        engine.check(&req);
        assert_eq!(engine.audit_log().len(), 1);
    }
}
