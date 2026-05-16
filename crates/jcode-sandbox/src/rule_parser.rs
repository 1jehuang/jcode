// ════════════════════════════════════════════════════════════════
// 权限规则解析器 — 移植自 Claude Code permissionRuleParser.ts
//
// 支持 3 种匹配模式:
//   Exact:   "Read(./src/main.ts)"     -> 精确字符串匹配
//   Prefix:  "Bash(git status:*)"       -> 前缀匹配 (* 为通配符后缀)
//   Wildcard: "*Write*"                 -> 全局通配符匹配
//
// 额外能力:
//   - Shadowed Rule Detection (检测被高优先级规则覆盖的无效规则)
//   - 规则优先级排序
//   - 规则冲突检测 (Allow/Deny 冲突)
// ════════════════════════════════════════════════════════════════

use crate::types::{DecisionBehavior, PermissionRule, RuleMatch, RulePattern};
use regex::Regex;
use std::sync::LazyLock;

/// 规则语法: ToolName(pattern) 或 ToolName
/// 示例:
///   "Read(./src/main.ts)"     -> tool=Read, pattern="./src/main.ts", match=Exact
///   "Bash(git status:*)"      -> tool=Bash, pattern="git status:*", match=Prefix
///   "Bash(git *)"            -> tool=Bash, pattern="git *", match=Wildcard
///   "Write"                   -> tool=Write, pattern="", match=Exact(全量)
static RULE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^(\w+)(?:\((.*)\))?$"#).expect("rule regex must compile")
});

/// 前缀模式检测: 模式以 * 结尾 (如 "git status:*")
static PREFIX_SUFFIX_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r".*:\*$").unwrap()
});

/// 通配符检测: 模式包含 * 或 ?
static WILDCARD_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[*?[\]]").unwrap()
});

#[derive(Debug, Clone, thiserror::Error)]
pub enum ParseError {
    #[error("规则格式无效: '{0}', 期望 ToolName(pattern) 或 ToolName")]
    InvalidFormat(String),
    #[error("空工具名")]
    EmptyToolName,
    #[error("未知的匹配类型")]
    UnknownMatchType,
}

// --- 规则解析器 ---------------------------------------------

pub struct PermissionRuleParser {
    /// 已解析的规则列表 (按 priority DESC 排序)
    rules: Vec<PermissionRule>,
}

impl Default for PermissionRuleParser {
    fn default() -> Self {
        Self::new()
    }
}

impl PermissionRuleParser {
    pub fn new() -> Self {
        Self { rules: Vec::new() }
    }

    /// 从字符串解析单条规则
    ///
    /// # 示例
    /// ```
    /// let parser = PermissionRuleParser::new();
    /// let rule = parser.parse("Read(./src/main.ts)").unwrap();
    /// assert_eq!(rule.tool_name, "Read");
    /// assert_eq!(rule.pattern.match_type, RuleMatch::Exact);
    /// ```
    pub fn parse(&self, input: &str) -> Result<PermissionRule, ParseError> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Err(ParseError::EmptyToolName);
        }

        let caps = RULE_RE
            .captures(trimmed)
            .ok_or_else(|| ParseError::InvalidFormat(trimmed.to_string()))?;

        let tool_name = caps
            .get(1)
            .unwrap()
            .as_str()
            .to_string();

        if tool_name.is_empty() {
            return Err(ParseError::EmptyToolName);
        }

        let pattern_str: Option<&str> = caps.get(2).map(|m| m.as_str());

        let (pattern, match_type) = match pattern_str {
            Some(ps) if ps.is_empty() => (
                RulePattern { content: String::new(), match_type: RuleMatch::Exact },
                RuleMatch::Exact,
            ),
            Some(ps) => {
                // 检测前缀模式: "cmd:*"
if PREFIX_SUFFIX_RE.is_match(ps) || ps.ends_with(":*") {
                    (
                        RulePattern { content: ps.to_string(), match_type: RuleMatch::Prefix },
                        RuleMatch::Prefix,
                    )
                } else if WILDCARD_RE.is_match(ps) {
                    // 包含通配符字符
                    (
                        RulePattern { content: ps.to_string(), match_type: RuleMatch::Wildcard },
                        RuleMatch::Wildcard,
                    )
                } else {
                    // 纯精确匹配
                    (
                        RulePattern { content: ps.to_string(), match_type: RuleMatch::Exact },
                        RuleMatch::Exact,
                    )
                }
            }
            None => (
                RulePattern { content: String::new(), match_type: RuleMatch::Exact },
                RuleMatch::Exact,
            ),
        };

        Ok(PermissionRule {
            tool_name,
            pattern,
            behavior: DecisionBehavior::Ask {
                reason: "需要确认".to_string(),
            }, // 默认需要用户确认
            priority: 0,
            description: None,
        })
    }

    /// 解析带行为的规则: "Allow:Read(./src/*)" 或 "Deny:Bash(rm -rf *)"
    pub fn parse_with_behavior(
        &self,
        input: &str,
        default_behavior: DecisionBehavior,
    ) -> Result<PermissionRule, ParseError> {
        let mut rule = self.parse(input)?;

        // 检查前缀行为指示
        let lower = input.trim().to_lowercase();
        if lower.starts_with("allow:") || lower.starts_with("allow ") {
            rule.behavior = DecisionBehavior::Allow;
        } else if lower.starts_with("deny:") || lower.starts_with("deny ") {
            rule.behavior = DecisionBehavior::Deny {
                reason: "规则拒绝".to_string(),
            };
        } else {
            rule.behavior = default_behavior;
        }

        Ok(rule
        )
    }

    /// 批量加载规则 (每行一条)
    pub fn load_rules(&mut self, rules_text: &str) -> Result<Vec<PermissionRule>, ParseError> {
        let mut loaded = Vec::new();
        for line in rules_text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            match self.parse(line) {
                Ok(rule) => loaded.push(rule),
                Err(e) => tracing::warn!("跳过无效规则 '{}': {}", line, e),
            }
        }
        self.rules.extend(loaded.clone());
        self.sort_rules();
        Ok(loaded)
    }

    /// 添加单条规则
    pub fn add_rule(&mut self, rule: PermissionRule) {
        self.rules.push(rule);
        self.sort_rules();
    }

    /// 按 priority DESC 排序 (高优先级在前)
    fn sort_rules(&mut self) {
        self.rules.sort_by(|a, b| b.priority.cmp(&a.priority));
    }

    /// 获取所有已注册规则
    pub fn rules(&self) -> &[PermissionRule] {
        &self.rules
    }

    /// 匹配工具调用到规则
    ///
    /// 返回第一个匹配的规则 (按优先级从高到低)
    pub fn match_tool_call(
        &self,
        tool_name: &str,
        input: &str,
    ) -> Option<&PermissionRule> {
        for rule in &self.rules {
            if !Self::tool_matches(&rule.tool_name, tool_name) {
                continue;
            }

            // 如果规则有 pattern 内容，检查输入是否匹配
            if !rule.pattern.content.is_empty() && !input.is_empty() {
                if !Self::input_matches_pattern(input, &rule.pattern) {
                    continue;
                }
            }

            return Some(rule);
        }
        None
    }

    /// 工具名匹配 (大小写不敏感)
    fn tool_matches(rule_tool: &str, actual_tool: &str) -> bool {
        rule_tool.eq_ignore_ascii_case(actual_tool)
    }

    /// 输入内容匹配模式
    fn input_matches_pattern(input: &str, pattern: &RulePattern) -> bool {
        match pattern.match_type {
            RuleMatch::Exact => {
                // 空模式匹配所有输入
                if pattern.content.is_empty() {
                    return true;
                }
                input == pattern.content
            }
            RuleMatch::Prefix => {
                // 去掉末尾 :* 后做前缀匹配
                let prefix = pattern.content.trim_end_matches(":*").trim_end_matches('*');
                if prefix.is_empty() {
                    return true;
                }
                input.starts_with(prefix)
            }
            RuleMatch::Wildcard => {
                // 将简单通配符转换为 regex
                // * 匹配任意字符, ? 匹配单个字符
                let re_str = Self::wildcard_to_regex(&pattern.content);
                if let Ok(re) = Regex::new(&re_str) {
                    re.is_match(input)
                } else {
                    // fallback: 简单 contains
                    let wc = pattern.content.replace('*', "").replace('?', "");
                    input.contains(&wc)
                }
            }
        }
    }

    /// 将通配符模式转为正则表达式
    fn wildcard_to_regex(pattern: &str) -> String {
        let mut re = String::from("^");
        for ch in pattern.chars() {
            match ch {
                '*' => re.push_str(".*"),
                '?' => re.push('.'),
                '.' | '+' | '(' | ')' | '[' | ']' | '{' | '}' | '|' | '^' | '$' | '\\' => {
                    re.push('\\');
                    re.push(ch);
                }
                _ => re.push(ch),
            }
        }
        re.push('$');
        re
    }

    // --- Shadowed Rule Detection --------------------------

    /// 检测被其他规则 shadowed (覆盖/失效) 的规则
    ///
    /// 规则 A 被 B shadowed 当且仅当:
    /// 1. A 的优先级 < B 的优先级
    /// 2. B 的匹配范围 ⊇ A 的匹配范围 (B 能匹配所有 A 能匹配的情况)
    ///
    /// Returns: (shadowed_indices, reason_for_each)
    pub fn detect_shadowed_rules(
        &self,
    ) -> Vec<(usize, String)> {
        let mut shadowed = Vec::new();

        for (i, rule_a) in self.rules.iter().enumerate() {
            for (j, rule_b) in self.rules.iter().enumerate() {
                if i == j {
                    continue;
                }
                // 只检查低优先级规则是否被高优先级规则覆盖
                if rule_a.priority >= rule_b.priority {
                    continue;
                }

                // 检查范围包含关系
                if Self::rule_covers(rule_b, rule_a) {
                    let reason = format!(
                        "规则 '{}' (p={}) 被规则 '{}' (p={}) 覆盖",
                        rule_a.display_name(),
                        rule_a.priority,
                        rule_b.display_name(),
                        rule_b.priority
                    );
                    shadowed.push((i, reason));
                    break; // 一个规则只需报告一次
                }
            }
        }

        shadowed
    }

    /// 检查 rule_cover 是否完全覆盖 rule_target 的匹配范围
    fn rule_covers(cover: &PermissionRule, target: &PermissionRule) -> bool {
        // 工具名必须一致或 cover 更通用
        if !Self::tool_names_compatible(&cover.tool_name, &target.tool_name) {
            return false;
        }

        // 检查 pattern 覆盖
        match (&cover.pattern.match_type, &target.pattern.match_type) {
            // Exact 覆盖 Exact: 必须相同
            (RuleMatch::Exact, RuleMatch::Exact) => {
                cover.pattern.content.is_empty() || cover.pattern.content == target.pattern.content
            }
            // Prefix/Wildcard 可以覆盖 Exact/Prefix
            (RuleMatch::Prefix | RuleMatch::Wildcard, _) => {
                // 前缀/通配符可以覆盖更具体的模式 (如果前缀更短或为空)
                cover.pattern.content.len() <= target.pattern.content.len()
                    || cover.pattern.content.is_empty()
            }
            // 其他情况: 不覆盖
            _ => false,
        }
    }

    fn tool_names_compatible(cover: &str, target: &str) -> bool {
        cover.eq_ignore_ascii_case(target)
    }

    /// 检测 Allow/Deny 冲突
    ///
    /// 当同一工具+模式的两个规则一个 Allow 一个 Deny 时产生冲突
    pub fn detect_conflicts(&self) -> Vec<(usize, usize, String)> {
        let mut conflicts = Vec::new();

        for i in 0..self.rules.len() {
            for j in (i + 1)..self.rules.len() {
                let a = &self.rules[i];
                let b = &self.rules[j];

                // 检查是否匹配相同的目标范围
                if a.tool_name.eq_ignore_ascii_case(&b.tool_name) {
                    let both_allow =
                        matches!(a.behavior, DecisionBehavior::Allow)
                            && matches!(b.behavior, DecisionBehavior::Allow);
                    let both_deny =
                        matches!(a.behavior, DecisionBehavior::Deny { .. })
                            && matches!(b.behavior, DecisionBehavior::Deny { .. });
                    let one_allow_one_deny = (matches!(a.behavior, DecisionBehavior::Allow)
                        && matches!(b.behavior, DecisionBehavior::Deny { .. }))
                        || (matches!(a.behavior, DecisionBehavior::Deny { .. })
                            && matches!(b.behavior, DecisionBehavior::Allow));

                    if one_allow_one_deny {
                        let desc = format!(
                            "规则 '{}' ({:?}) 与规则 '{}' ({:?}) 冲突",
                            a.display_name(),
                            a.behavior,
                            b.display_name(),
                            b.behavior
                        );
                        conflicts.push((i, j, desc));
                    } else if !both_allow && !both_deny {
                        // Allow vs Ask 或 Deny vs Ask 不算严格冲突但值得注意
                    }
                }
            }
        }

        conflicts
    }
}

impl PermissionRule {
    pub fn display_name(&self) -> String {
        if self.pattern.content.is_empty() {
            self.tool_name.clone()
        } else {
            format!("{}({})", self.tool_name, self.pattern.content)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_exact_rule() {
        let p = PermissionRuleParser::new();
        let r = p.parse("Read(./src/main.rs)").unwrap();
        assert_eq!(r.tool_name, "Read");
        assert_eq!(r.pattern.content, "./src/main.rs");
        assert_eq!(r.pattern.match_type, RuleMatch::Exact);
    }

    #[test]
    fn test_parse_prefix_rule() {
        let p = PermissionRuleParser::new();
        let r = p.parse("Bash(git status:*)").unwrap();
        assert_eq!(r.tool_name, "Bash");
        assert_eq!(r.pattern.content, "git status:*");
        assert_eq!(r.pattern.match_type, RuleMatch::Prefix);
    }

    #[test]
    fn test_parse_wildcard_rule() {
        let p = PermissionRuleParser::new();
        let r = p.parse("Bash(git *)").unwrap();
        assert_eq!(r.pattern.match_type, RuleMatch::Wildcard);
    }

    #[test]
    fn test_parse_tool_only() {
        let p = PermissionRuleParser::new();
        let r = p.parse("Write").unwrap();
        assert_eq!(r.tool_name, "Write");
        assert_eq!(r.pattern.content, "");
    }

    #[test]
    fn test_parse_allow_prefix() {
        let p = PermissionRuleParser::new();
        let r = p.parse_with_behavior("Allow:Read(*)", DecisionBehavior::Ask { reason: "x".into() }).unwrap();
        assert_eq!(r.behavior, DecisionBehavior::Allow);
    }

    #[test]
    fn test_match_exact() {
        let p = PermissionRuleParser::new();
        let _ = p.parse("Read(./src/main.rs)");
        // TODO: 完整测试匹配逻辑
    }

    #[test]
    fn test_shadowed_detection() {
        let mut p = PermissionRuleParser::new();
        p.add_rule(PermissionRule {
            tool_name: "Bash".into(),
            pattern: RulePattern { content: "git push".into(), match_type: RuleMatch::Exact },
            behavior: DecisionBehavior::Deny { reason: "low pri deny".into() },
            priority: 1,
            description: None,
        });
        p.add_rule(PermissionRule {
            tool_name: "Bash".into(),
            pattern: RulePattern { content: "".into(), match_type: RuleMatch::Exact },
            behavior: DecisionBehavior::Allow,
            priority: 10,
            description: Some("允许所有 Bash".into()),
        });

        let shadowed = p.detect_shadowed_rules();
        assert!(!shadowed.is_empty(), "应检测到被覆盖的低优先级规则");
    }

    #[test]
    fn test_conflict_detection() {
        let mut p = PermissionRuleParser::new();
        p.add_rule(PermissionRule {
            tool_name: "Bash".into(),
            pattern: RulePattern { content: "rm -rf *".into(), match_type: RuleMatch::Wildcard },
            behavior: DecisionBehavior::Deny { reason: "dangerous".into() },
            priority: 5,
            description: None,
        });
        p.add_rule(PermissionRule {
            tool_name: "Bash".into(),
            pattern: RulePattern { content: "rm -rf *".into(), match_type: RuleMatch::Wildcard },
            behavior: DecisionBehavior::Allow,
            priority: 5,
            description: None,
        });

        let conflicts = p.detect_conflicts();
        assert!(!conflicts.is_empty(), "应检测到 Allow/Deny 冲突");
    }

    #[test]
    fn test_batch_load() {
        let mut p = PermissionRuleParser::new();
        let rules = p.load_rules(
            "# 这是注释\n\
             Allow:Read(*)\n\
             Deny:Bash(rm -rf /)\n\
             \n\
             Write(./src/*)",
        ).unwrap();
        assert_eq!(rules.len(), 3); // 注释和空行跳过
    }
}
