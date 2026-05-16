//! # Heredoc语法解析器
//!
//! 支持Shell heredoc语法的智能补全：
//! - **标准heredoc**: `<<EOF`
//! - **引号heredoc**: `<<'EOF'` (禁止变量展开)
//! - **缩进heredoc**: `<<-EOF` (允许内容缩进)
//! - **嵌套heredoc检测**
//!
//! ## 语法示例
//!
//! ```bash
//! cat << EOF          # 标准heredoc
//! cat << 'EOF'        # 引号heredoc（不展开变量）
//! cat <<-EOF           # 缩进heredoc（忽略前导tab）
//! ```

use crate::completion::bash::{CompletionSuggestion, CompletionKind, CompletionContext};
use regex::Regex;
use std::collections::HashMap;

/// Heredoc类型
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeredocType {
    /// 标准 <<EOF (变量会展开)
    Standard,
    /// 引号 <<'EOF' (变量不展开)
    Quoted,
    /// 缩进 <<-EOF (允许Tab缩进)
    Indented,
}

/// 解析后的Heredoc信息
#[derive(Debug, Clone)]
pub struct HeredocInfo {
    /// heredoc类型
    pub heredoc_type: HeredocType,
    
    /// 分隔符名称 (如 "EOF", "END", "SCRIPT")
    pub delimiter: String,
    
    /// 在输入中的起始位置
    pub start_position: usize,
    
    /// 是否需要补全分隔符
    pub needs_delimiter_completion: bool,
}

/// Heredoc补全建议生成器
pub struct HeredocParser {
    /// 常用分隔符列表
    common_delimiters: Vec<HeredocDelimiter>,
    
    /// 预编译正则
    heredoc_regex: Regex,
    delimiter_regex: Regex,
}

/// Heredoc分隔符定义
#[derive(Debug, Clone)]
struct HeredocDelimiter {
    name: String,
    description: String,
    usage_context: Option<String>,  // 使用场景
}

impl Default for HeredocParser {
    fn default() -> Self {
        let mut delimiters = vec![
            // 通用分隔符
            HeredocDelimiter {
                name: "EOF".to_string(),
                description: "End of File".to_string(),
                usage_context: None,
            },
            HeredocDelimiter {
                name: "END".to_string(),
                description: "End".to_string(),
                usage_context: None,
            },
            HeredocDelimiter {
                name: "HEREDOC_END".to_string(),
                description: "Heredoc end marker".to_string(),
                usage_context: None,
            },
            
            // 脚本相关
            HeredocDelimiter {
                name: "SCRIPT".to_string(),
                description: "Script content".to_string(),
                usage_context: Some("shell script generation".to_string()),
            },
            HeredocDelimiter {
                name: "CONFIG".to_string(),
                description: "Configuration file".to_string(),
                usage_context: Some("config file creation".to_string()),
            },
            HeredocDelimiter {
                name: "JSON".to_string(),
                description: "JSON data".to_string(),
                usage_context: Some("JSON input".to_string()),
            },
            HeredocDelimiter {
                name: "SQL".to_string(),
                description: "SQL query".to_string(),
                usage_context: Some("database queries".to_string()),
            },
            HeredocDelimiter {
                name: "HTML".to_string(),
                description: "HTML template".to_string(),
                usage_context: Some("web content".to_string()),
            },
            
            // 特定用途
            HeredocDelimiter {
                name: "SSH_KEY".to_string(),
                description: "SSH public key".to_string(),
                usage_context: Some("SSH key setup".to_string()),
            },
            HeredocDelimiter {
                name: "CRON".to_string(),
                description: "Cron job definition".to_string(),
                usage_context: Some("cron configuration".to_string()),
            },
            HeredocDelimiter {
                name: "DOCKERFILE".to_string(),
                description: "Dockerfile content".to_string(),
                usage_context: Some("Docker build context".to_string()),
            },
        ];

        // 按使用频率排序
        delimiters.sort_by(|a, b| {
            // 常用的排在前面
            let a_score = match a.name.as_str() {
                "EOF" | "END" => 0,
                "SCRIPT" | "CONFIG" | "JSON" => 1,
                _ => 2,
            };
            let b_score = match b.name.as_str() {
                "EOF" | "END" => 0,
                "SCRIPT" | "CONFIG" | "JSON" => 1,
                _ => 2,
            };
            a_score.cmp(&b_score)
        });

        Self {
            common_delimiters: delimiters,
            heredoc_regex: Regex::new(r"<<(-?)\s*(['"]?)(\w+)?\2").unwrap(),
            delimiter_regex: Regex::new(r"^\s*(\w+)\s*$").unwrap(),
        }
    }
}

impl HeredocParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// 解析输入中的heredoc语法并返回补全建议
    pub fn parse_and_suggest(
        &self,
        input: &str,
        cursor_pos: usize,
    ) -> Vec<CompletionSuggestion> {
        let mut suggestions = vec![];

        // 检测是否在heredoc上下文中
        if let Some(heredoc_info) = self.detect_heredoc(input, cursor_pos) {
            // 如果需要补充分隔符
            if heredoc_info.needs_delimiter_completion {
                suggestions.extend(self.suggest_delimiters(&heredoc_info));
            }

            // 提供heredoc模板内容建议
            suggestions.extend(self.suggest_content_templates(&heredoc_info));
        } else {
            // 检查是否应该建议插入heredoc
            if self.should_suggest_heredoc(input, cursor_pos) {
                suggestions.extend(self.suggest_heredoc_insertion());
            }
        }

        suggestions
    }

    /// 检测当前是否在heredoc上下文中
    pub fn detect_heredoc(&self, input: &str, cursor_pos: usize) -> Option<HeredocInfo> {
        // 查找光标前的<<符号
        let before_cursor = &input[..cursor_pos.min(input.len())];
        
        // 匹配 << 或 <<- 或 <<' 或 <<"
        for cap in self.heredoc_regex.captures_iter(before_cursor) {
            let full_match = cap.get(0)?;
            let start = full_match.start();
            let matched_text = full_match.as_str();

            // 确定类型
            let heredoc_type = if matched_text.starts_with("<<-") {
                HeredocType::Indented
            } else if matched_text.contains('\'') || matched_text.contains('"') {
                HeredocType::Quoted
            } else {
                HeredocType::Standard
            };

            // 提取已有分隔符（如果有）
            let existing_delimiter = cap.get(3).map(|m| m.as_str().to_string());

            return Some(HeredocInfo {
                heredoc_type,
                delimiter: existing_delimiter.unwrap_or_default(),
                start_position: start,
                needs_delimiter_completion: existing_delimiter.is_none() || 
                    (existing_delimiter.as_ref().map_or(true, |d| d.is_empty())),
            });
        }

        None
    }

    /// 是否应该建议插入heredoc
    fn should_suggest_heredoc(&self, input: &str, cursor_pos: usize) -> bool {
        // 检查是否在cat/tee/命令后面，且没有重定向到文件
        let before_cursor = &input[..cursor_pos.min(input.len())];
        
        // 简单启发式：如果最后一个完整单词是cat或类似命令
        let last_word_match = Regex::new(r"(?:^|\s)(cat|tee|openssl|base64|python|ruby|perl|node)\s*$")
            .unwrap()
            .find(before_cursor);

        last_word_match.is_some()
    }

    /// 建议分隔符
    fn suggest_delimiters(&self, info: &HeredocInfo) -> Vec<CompletionSuggestion> {
        self.common_delimiters.iter().map(|delim| {
            let prefix = match info.heredoc_type {
                HeredocType::Standard => "<<",
                HeredocType::Quoted => "<<'",
                HeredocType::Indented => "<<-",
            };

            CompletionSuggestion {
                text: format!("{}{}", prefix, delim.name),
                display_text: format!("{}{}", prefix, delim.name),
                description: format!(
                    "{} - {}",
                    delim.name,
                    delim.description
                ),
                kind: CompletionKind::Other,
                priority: 90,
                metadata: {
                    let mut meta = HashMap::new();
                    meta.insert("type".to_string(), "heredoc_delimiter".to_string());
                    meta.insert("delimiter_name".to_string(), delim.name.clone());
                    if let Some(ctx) = &delim.usage_context {
                        meta.insert("context".to_string(), ctx.clone());
                    }
                    meta
                },
            }
        }).collect()
    }

    /// 建议heredoc内容模板
    fn suggest_content_templates(&self, info: &HeredocInfo) -> Vec<CompletionSuggestion> {
        let templates = match info.delimiter.to_uppercase().as_str() {
            "SCRIPT" | "" => vec![
                ("#!/bin/bash\nset -euo pipefail\n\n${1:# script body}\n", "Bash script template"),
                ("#!/usr/bin/env python3\n\"\"\"\n${1:description}\n\"\"\"\n\n${2:# code}\n", "Python script"),
            ],
            "CONFIG" | "JSON" => vec![
                ("{\n  \"${1:key}\": \"${2:value}\"\n}\n", "JSON object"),
                ("[${1:item1}, ${2:item2}]\n", "JSON array"),
            ],
            "SQL" => vec![
                ("SELECT * FROM ${1:table}\nWHERE ${2:condition};\n", "SQL SELECT"),
                ("INSERT INTO ${1:table} (${2:columns})\nVALUES (${3:values});\n", "SQL INSERT"),
            ],
            "HTML" => vec![
                ("<html>\n<head>\n    <title>${1:title}</title>\n</head>\n<body>\n    ${2:content}\n</body>\n</html>", "HTML5 template"),
            ],
            "DOCKERFILE" => vec![
                ("FROM ${1:image}:${2:tag}\nWORKDIR /app\nCOPY . .\nRUN ${3:build_command}\nCMD [\"${4:command}\"]", "Dockerfile"),
            ],
            "SSH_KEY" => vec![
                ("ssh-rsa AAAA...${1:user}@${2:host}", "SSH public key placeholder"),
            ],
            _ => vec![
                ("${1:content here}\n", "Generic text block"),
            ],
        };

        templates.into_iter().enumerate().map(|(i, (template, desc))| {
            CompletionSuggestion {
                text: format!("\n{}\n{}", info.delimiter, template),
                display_text: format!("{} template #{}", info.delimiter, i + 1),
                description: desc.to_string(),
                kind: CompletionKind::Other,
                priority: 70 - (i as u8), // 降低优先级
                metadata: {
                    let mut meta = HashMap::new();
                    meta.insert("type".to_string(), "heredoc_template".to_string());
                    meta.insert("template_preview".to_string(), 
                        template.lines().take(3).collect::<Vec<_>>().join("\\n"));
                    meta
                },
            }
        }).collect()
    }

    /// 建议插入heredoc
    fn suggest_heredoc_insertion(&self) -> Vec<CompletionSuggestion> {
        vec![
            CompletionSuggestion {
                text: " << EOF\n${1:content here}\nEOF".to_string(),
                display_text: "<< EOF ...".to_string(),
                description: "Insert standard heredoc".to_string(),
                kind: CompletionKind::Other,
                priority: 85,
                metadata: HashMap::new(),
            },
            CompletionSuggestion {
                text: " << 'EOF'\n${1:raw content (no variable expansion)}\nEOF".to_string(),
                display_text: "<< 'EOF' ...".to_string(),
                description: "Insert quoted heredoc (no expansion)".to_string(),
                kind: CompletionKind::Other,
                priority: 80,
                metadata: HashMap::new(),
            },
            CompletionSuggestion {
                text: " <<-EOF\n\t${1:indented content}\nEOF".to_string(),
                display_text: "<<-EOF ...".to_string(),
                description: "Insert indented heredoc".to_string(),
                kind: CompletionKind::Other,
                priority: 75,
                metadata: HashMap::new(),
            },
        ]
    }

    /// 验证heredoc是否正确闭合
    pub fn validate_heredoc_closing(&self, input: &str) -> Result<(), HeredocError> {
        // 查找所有未闭合的heredoc
        let open_pattern = Regex::new(r"<<(-?)['\"]?(\w+)['\"]?\s*").unwrap();
        
        let mut open_heredocs: Vec<(String, HeredocType)> = vec![];
        
        for cap in open_pattern.captures_iter(input) {
            let is_indented = cap.get(1).map_or(false, |m| m.as_str() == "-");
            let delimiter = cap.get(2).map(|m| m.as_str().to_string()).unwrap_or_default();
            
            let htype = if is_indented { HeredocType::Indented } else { HeredocType::Standard };
            open_heredocs.push((delimiter, htype));
        }

        // 简化版：检查是否有匹配的结束标记
        // （完整实现需要状态机来处理嵌套和转义）
        for (delimiter, _) in &open_heredocs {
            let close_pattern = Regex::new(&format!(r"^{}\s*$", regex::escape(delimiter))).unwrap();
            
            if !close_pattern.is_match(input) {
                return Err(HeredocError::UnclosedDelimiter(delimiter.clone()));
            }
        }

        Ok(())
    }
}

/// Heredoc错误类型
#[derive(Debug, Clone)]
pub enum HeredocError {
    UnclosedDelimiter(String),
    InvalidSyntax(String),
    NestedNotSupported(String),
}

impl std::fmt::Display for HeredocError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HeredocError::UnclosedDelimiter(delim) => {
                write!(f, "Unclosed heredoc delimiter: {}", delim)
            }
            HeredocError::InvalidSyntax(msg) => {
                write!(f, "Invalid heredoc syntax: {}", msg)
            }
            HeredocError::NestedNotSupported(msg) => {
                write!(f, "Nested heredoc not supported: {}", msg)
            }
        }
    }
}

// ==========================================
// Shell Alias 展开器
// ==========================================

/// Shell别名管理器
pub struct AliasManager {
    /// 别名映射: alias -> 完整命令
    aliases: HashMap<String, AliasDefinition>,
    
    /// 加载的文件路径
    loaded_files: Vec<std::path::PathBuf>,
}

/// 别名定义
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AliasDefinition {
    /// 展开后的完整命令
    pub expansion: String,
    
    /// 描述（可选）
    pub description: Option<String>,
    
    /// 来源文件
    pub source_file: Option<String>,
    
    /// 是否为全局别名
    pub is_global: bool,
}

impl Default for AliasManager {
    fn default() -> Self {
        let mut manager = Self {
            aliases: HashMap::new(),
            loaded_files: vec![],
        };

        // 注册常见内置别名
        manager.register_builtin_aliases();
        manager
    }
}

impl AliasManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册内置常用别名
    fn register_builtin_aliases(&mut self) {
        let builtin_aliases = [
            // Git别名
            ("g", "git", Some("Git shorthand")),
            ("gs", "git status", Some("Show working tree status")),
            ("ga", "git add", Some("Add file contents to index")),
            ("gc", "git commit", Some("Record changes to repository")),
            ("gp", "git push", Some("Update remote refs along with associated objects")),
            ("gl", "git pull", None),
            ("gd", "git diff", None),
            ("gb", "git branch", None),
            ("gco", "git checkout", None),
            
            // 目录导航
            ("..", "cd ..", Some("Go up one directory")),
            ("...",", "cd ../..", Some("Go up two directories")),
            ("ll", "ls -alF", Some("Long format listing")),
            ("la", "ls -A", Some("List all except . and ..")),
            ("lt", "ls --human-readable --size -1 -S --classify", Some("List by size")),
            
            // 安全操作
            ("rm", "rm -iv", Some("Interactive remove with verbose")),
            ("mv", "mv -iv", Some("Interactive move with verbose")),
            ("cp", "cp -iv", Some("Interactive copy with verbose")),
            ("mkdir", "mkdir -pv", Some("Make directories with parents")),
            
            // Docker别名
            ("d", "docker", Some("Docker shorthand")),
            ("dp", "docker ps", Some("List containers")),
            ("di", "docker images", Some("List images")),
            ("dr", "docker run", Some("Run container")),
            
            // 其他实用工具
            ("cls", "clear", Some("Clear screen")),
            ("h", "history", Some("Command history")),
            ("dfh", "df -h", Some("Disk free human readable")),
            ("duh", "du -h --max-depth=1", Some("Disk usage summary")),
        ];

        for (alias, expansion, desc) in builtin_aliases.iter() {
            self.aliases.insert(alias.to_string(), AliasDefinition {
                expansion: expansion.to_string(),
                description: desc.map(|d| d.to_string()),
                source_file: None,
                is_global: true,
            });
        }
    }

    /// 从文件加载别名（.bashrc, .zshrc等）
    pub fn load_from_file(&mut self, path: &std::path::Path) -> Result<usize, String> {
        if !path.exists() {
            return Err(format!("File not found: {:?}", path));
        }

        let content = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        let alias_regex = Regex::new(r#"^\s*alias\s+(?:['"]?)([\w.-]+)(?:['"]?)=(?:['"])(.+)(?:['"])"#)
            .unwrap();

        let mut count = 0usize;

        for line in content.lines() {
            if let Some(cap) = alias_regex.captures(line) {
                let name = cap.get(1).expect("alias regex must have group 1").as_str().to_string();
                let expansion = cap.get(2).expect("alias regex must have group 2").as_str().to_string();

                self.aliases.insert(name.clone(), AliasDefinition {
                    expansion,
                    description: None,
                    source_file: path.file_name().map(|f| f.to_string_lossy().to_string()),
                    is_global: false,
                });
                
                count += 1;
            }
        }

        self.loaded_files.push(path.to_path_buf());
        Ok(count)
    }

    /// 展开别名
    pub fn expand_alias(&self, input: &str) -> String {
        // 提取第一个词作为潜在的别名
        let first_word = input.split_whitespace().next().unwrap_or("");

        if let Some(alias_def) = self.aliases.get(first_word) {
            // 替换别名为完整命令
            let rest = input[first_word.len()..].trim_start();
            format!("{} {}", alias_def.expansion, rest)
        } else {
            input.to_string()
        }
    }

    /// 检查是否是别名
    pub fn is_alias(&self, word: &str) -> bool {
        self.aliases.contains_key(word)
    }

    /// 获取别名定义
    pub fn get_alias(&self, name: &str) -> Option<&AliasDefinition> {
        self.aliases.get(name)
    }

    /// 获取所有别名
    pub fn list_aliases(&self) -> Vec<&str> {
        self.aliases.keys().map(|s| s.as_str()).collect()
    }

    /// 搜索别名
    pub fn search(&self, query: &str) -> Vec<AliasCompletion> {
        let query_lower = query.to_lowercase();
        
        self.aliases.iter()
            .filter(|(name, def)| {
                name.to_lowercase().contains(&query_lower)
                    || def.expansion.to_lowercase().contains(&query_lower)
                    || def.description.as_ref()
                        .map_or(false, |d| d.to_lowercase().contains(&query_lower))
            })
            .map(|(name, def)| AliasCompletion {
                name: name.clone(),
                expansion: def.expansion.clone(),
                description: def.description.clone(),
                is_builtin: def.is_global,
            })
            .collect()
    }

    /// 统计信息
    pub fn statistics(&self) -> AliasStatistics {
        let builtin_count = self.aliases.values().filter(|a| a.is_global).count();
        let custom_count = self.aliases.len() - builtin_count;

        AliasStatistics {
            total_aliases: self.aliases.len(),
            builtin_aliases: builtin_count,
            custom_aliases: custom_count,
            loaded_files: self.loaded_files.len(),
        }
    }
}

/// 别名补全结果
#[derive(Debug, Clone)]
pub struct AliasCompletion {
    pub name: String,
    pub expansion: String,
    pub description: Option<String>,
    pub is_builtin: bool,
}

/// 别名统计信息
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AliasStatistics {
    pub total_aliases: usize,
    pub builtin_aliases: usize,
    pub custom_aliases: usize,
    pub loaded_files: usize,
}

// ==========================================
// 单元测试
// ==========================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_standard_heredoc() {
        let parser = HeredocParser::new();
        
        let info = parser.detect_heredoc("cat << ", 6);
        assert!(info.is_some());
        
        let info = info.unwrap();
        assert_eq!(info.heredoc_type, HeredocType::Standard);
        assert!(info.needs_delimiter_completion);
    }

    #[test]
    fn test_detect_quoted_heredoc() {
        let parser = HeredocParser::new();
        
        let info = parser.detect_heredoc("cat << '", 7);
        assert!(info.is_some());
        
        let info = info.unwrap();
        assert_eq!(info.heredoc_type, HeredocType::Quoted);
    }

    #[test]
    fn test_detect_indented_heredoc() {
        let parser = HeredocParser::new();
        
        let info = parser.detect_heredoc("cat <<-", 7);
        assert!(info.is_some());
        
        let info = info.unwrap();
        assert_eq!(info.heredoc_type, HeredocType::Indented);
    }

    #[test]
    fn test_delimiter_suggestions() {
        let parser = HeredocParser::new();
        let info = HeredocInfo {
            heredoc_type: HeredocType::Standard,
            delimiter: String::new(),
            start_position: 4,
            needs_delimiter_completion: true,
        };

        let suggestions = parser.suggest_delimiters(&info);
        
        assert!(!suggestions.is_empty());
        
        // 应该包含常见的EOF
        let has_eof = suggestions.iter().any(|s| s.text.contains("EOF"));
        assert!(has_eof);
    }

    #[test]
    fn test_should_suggest_for_cat_command() {
        let parser = HeredocParser::new();
        
        assert!(parser.should_suggest_heredoc("cat ", 4));
        assert!(parser.should_suggest_heredoc("tee ", 4));
        assert!(!parser.should_suggest_heredoc("echo ", 5));
    }

    #[test]
    fn test_alias_expansion() {
        let manager = AliasManager::new();
        
        assert_eq!(manager.expand_alias("gs"), "git status");
        assert_eq!(manager.expand_alias("ll"), "ls -alF");
        assert_eq!(manager.expand_alias("unknown command"), "unknown command");
    }

    #[test]
    fn test_alias_detection() {
        let manager = AliasManager::new();
        
        assert!(manager.is_alias("g"));
        assert!(manager.is_alias("ll"));
        assert!(!manager.is_alias("not_an_alias"));
    }

    #[test]
    fn test_alias_search() {
        let manager = AliasManager::new();
        
        let results = manager.search("git");
        
        assert!(!results.is_empty());
        
        // 应该包含git相关的别名
        let has_git_alias = results.iter().any(|a| a.expansion.starts_with("git"));
        assert!(has_git_alias);
    }

    #[test]
    fn test_builtin_alias_statistics() {
        let manager = AliasManager::new();
        
        let stats = manager.statistics();
        
        assert!(stats.total_aliases > 20);  // 应该有20+内置别名
        assert!(stats.builtin_aliases > 20);
        assert_eq!(stats.custom_aliases, 0);  // 还没加载自定义
    }

    #[test]
    fn test_heredoc_validation_success() {
        let parser = HeredocParser::new();
        
        let valid_input = r#"cat << EOF
some content
EOF"#;
        
        assert!(parser.validate_heredoc_closing(valid_input).is_ok());
    }

    #[test]
    fn test_heredoc_validation_failure() {
        let parser = HeredocParser::new();
        
        let invalid_input = "cat << EOF\nsome content\n";  // 缺少结束标记
        
        let result = parser.validate_heredoc_closing(invalid_input);
        assert!(result.is_err());
        
        match result.err().unwrap() {
            HeredocError::UnclosedDelimiter(delim) => {
                assert_eq!(delim, "EOF");
            }
            other => panic!("Expected UnclosedDelimiter error, got {:?}", other),
        }
    }
}
