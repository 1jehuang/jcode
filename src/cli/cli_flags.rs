//! CLI Flags (命令行标志) 解析器
//!
//! Claude Code兼容的CLI选项解析
//!
//! ## 支持的标志 (Phase 1)

use std::collections::HashSet;

/// CLI配置标志
#[derive(Debug, Clone, Default)]
pub struct CliFlags {
    // === 核心模式 ===
    /// Print模式 (-p)
    pub print_mode: bool,
    
    /// 继续上次会话 (-c)
    pub continue_session: bool,
    
    /// 恢复会话 (-r)
    pub resume_session: Option<String>,
    
    // === 工作目录 ===
    /// 额外工作目录 (--add-dir)
    pub additional_dirs: Vec<String>,
    
    // === 模型配置 ===
    /// 模型名称 (--model)
    pub model: Option<String>,
    
    /// 回退模型 (--fallback-model)
    pub fallback_model: Option<String>,
    
    // === 权限控制 ===
    /// 跳过权限提示 (--dangerously-skip-permissions)
    pub skip_permissions: bool,
    
    /// 允许跳过权限作为选项 (--allow-dangerously-skip-permissions)
    pub allow_skip_permissions: bool,
    
    /// 工具白名单 (--allowedTools)
    pub allowed_tools: Vec<String>,
    
    /// 工具黑名单 (--disallowedTools)
    pub disallowed_tools: Vec<String>,
    
    // === 系统提示 ===
    /// 追加系统提示 (--append-system-prompt)
    pub append_system_prompt: Option<String>,
    
    /// 追加系统提示文件 (--append-system-prompt-file)
    pub append_system_prompt_file: Option<String>,
    
    // === I/O控制 ===
    /// 静默模式 (--quiet)
    pub quiet: bool,
    
    /// 详细输出 (--verbose)
    pub verbose: bool,
    
    /// JSON输出 (--json)
    pub json_output: bool,
    
    /// NDJSON流式输出 (--ndjson)
    pub ndjson_output: bool,
    
    // === 调试 ===
    /// 调试模式 (--debug)
    pub debug_mode: bool,
    
    /// 调试类别过滤
    pub debug_categories: HashSet<String>,
    
    // === 高级功能 ===
    /// Chrome集成 (--chrome)
    pub chrome_integration: bool,
    
    /// 指定代理 (--agent)
    pub agent: Option<String>,
    
    /// 动态定义子代理 (--agents)
    pub agents_json: Option<serde_json::Value>,
    
    /// 禁用斜杠命令 (--disable-slash-commands)
    pub disable_slash_commands: bool,
    
    /// 分支会话 (--fork-session)
    pub fork_session: bool,
}

impl CliFlags {
    /// 创建新的默认标志
    pub fn new() -> Self {
        Self::default()
    }
    
    /// 检查是否启用调试模式
    pub fn is_debug_enabled(&self) -> bool {
        self.debug_mode || !self.debug_categories.is_empty()
    }
    
    /// 检查是否启用特定调试类别
    pub fn is_debug_category(&self, category: &str) -> bool {
        if self.debug_mode {
            return true; // 全部启用
        }
        
        if self.debug_categories.is_empty() {
            return false;
        }
        
        // 支持排除语法: "!statsig,!file"
        for cat in &self.debug_categories {
            if cat.starts_with('!') {
                let excluded = &cat[1..];
                if category.contains(excluded) {
                    return false;
                }
            } else if category.contains(cat.as_str()) {
                return true;
            }
        }
        
        false
    }
    
    /// 获取有效工具列表 (白名单 - 黑名单)
    pub fn get_effective_tools(&self) -> Option<Vec<String>> {
        if !self.allowed_tools.is_empty() {
            Some(self.allowed_tools.clone())
        } else if !self.disallowed_tools.is_empty() {
            None // 使用None表示"使用默认但排除黑名单"
        } else {
            None
        }
    }
}

/// 从命令行参数解析标志
pub fn parse_cli_flags(args: &[String]) -> CliFlags {
    let mut flags = CliFlags::new();
    let mut i = 0;
    
    while i < args.len() {
        let arg = &args[i];
        
        match arg.as_str() {
            // === 核心模式 ===
            "-p" | "--print" => {
                flags.print_mode = true;
            }
            
            "-c" | "--continue" => {
                flags.continue_session = true;
            }
            
            "-r" | "--resume" => {
                i += 1;
                if i < args.len() {
                    flags.resume_session = Some(args[i].clone());
                }
            }
            
            // === 工作目录 ===
            "--add-dir" => {
                i += 1;
                while i < args.len() && !args[i].starts_with('-') {
                    flags.additional_dirs.push(args[i].clone());
                    i += 1;
                }
                continue; // 已经递增了i
            }
            
            // === 模型配置 ===
            "-m" | "--model" => {
                i += 1;
                if i < args.len() {
                    flags.model = Some(args[i].clone());
                }
            }
            
            "--fallback-model" => {
                i += 1;
                if i < args.len() {
                    flags.fallback_model = Some(args[i].clone());
                }
            }
            
            // === 权限控制 ===
            "--dangerously-skip-permissions" | "-y" => {
                flags.skip_permissions = true;
            }
            
            "--allow-dangerously-skip-permissions" => {
                flags.allow_skip_permissions = true;
            }
            
            "--allowedTools" => {
                i += 1;
                if i < args.len() {
                    parse_tool_patterns(&args[i], &mut flags.allowed_tools);
                }
            }
            
            "--disallowedTools" => {
                i += 1;
                if i < args.len() {
                    parse_tool_patterns(&args[i], &mut flags.disallowed_tools);
                }
            }
            
            // === 系统提示 ===
            "--append-system-prompt" => {
                i += 1;
                if i < args.len() {
                    flags.append_system_prompt = Some(args[i].clone());
                }
            }
            
            "--append-system-prompt-file" => {
                i += 1;
                if i < args.len() {
                    flags.append_system_prompt_file = Some(args[i].clone());
                }
            }
            
            // === I/O控制 ===
            "-q" | "--quiet" => {
                flags.quiet = true;
            }
            
            "-v" | "--verbose" => {
                flags.verbose = true;
            }
            
            "--json" => {
                flags.json_output = true;
            }
            
            "--ndjson" => {
                flags.ndjson_output = true;
            }
            
            // === 调试 ===
            "--debug" => {
                flags.debug_mode = true;
                
                // 可选: 指定调试类别
                if i + 1 < args.len() && !args[i + 1].starts_with('-') {
                    i += 1;
                    let categories: Vec<&str> = args[i].split(',').collect();
                    for cat in categories {
                        flags.debug_categories.insert(cat.trim().to_string());
                    }
                    flags.debug_mode = false; // 有具体类别时不全局启用
                }
            }
            
            // === 高级功能 ===
            "--chrome" => {
                flags.chrome_integration = true;
            }
            
            "--agent" => {
                i += 1;
                if i < args.len() {
                    flags.agent = Some(args[i].clone());
                }
            }
            
            "--agents" => {
                i += 1;
                if i < args.len() {
                    if let Ok(json) = serde_json::from_str(&args[i]) {
                        flags.agents_json = Some(json);
                    }
                }
            }
            
            "--disable-slash-commands" => {
                flags.disable_slash_commands = true;
            }
            
            "--fork-session" => {
                flags.fork_session = true;
            }
            
            _ => {
                // 忽略未知参数 (由主参数处理器处理)
            }
        }
        
        i += 1;
    }
    
    flags
}

/// 解析工具模式字符串
fn parse_tool_patterns(patterns_str: &str, target: &mut Vec<String>) {
    // 支持空格分隔或引号包裹的模式
    for pattern in patterns_str.split_whitespace() {
        let cleaned = pattern.trim_matches('"').trim_matches('\'');
        if !cleaned.is_empty() {
            target.push(cleaned.to_string());
        }
    }
}

/// 生成帮助文本中的标志说明
pub fn generate_flags_help_text() -> String {
    r#"## CLI Flags Reference

### Core Modes
```
-p, --print              Print mode (non-interactive)
-c, --continue           Continue last session
-r, --resume <session>   Resume session by name/ID
```

### Working Directory
```
--add-dir <path>         Add extra working directory (repeatable)
```

### Model Configuration
```
-m, --model <name>       Specify AI model
--fallback-model <name>  Fallback model when primary is overloaded
```

### Permission Control
```
-y, --dangerously-skip-permissions  Skip all permission prompts
--allow-dangerously-skip-permissions  Allow skip option in UI
--allowedTools <patterns>     Tools that can run without asking
--disallowedTools <patterns>   Tools to remove from context
```

### System Prompt
```
--append-system-prompt <text>      Append custom text to system prompt
--append-system-prompt-file <file> Load prompt from file
```

### I/O Control
```
-q, --quiet             Suppress non-error output
-v, --verbose           Enable verbose output
--json                  Output in JSON format
--ndjson                 Output as newline-delimited JSON events
```

### Debug
```
--debug [categories]    Enable debug mode with optional category filter
                         Examples: "api,hooks", "!statsig,!file"
```

### Advanced
```
--chrome                Enable Chrome browser integration
--agent <name>          Use specific agent
--agents <json>         Define agents dynamically
--disable-slash-commands  Disable all slash commands
--fork-session          Create new session ID when resuming
```
"#.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_parse_print_flag() {
        let args = vec!["carpai".to_string(), "-p".to_string(), "query".to_string()];
        let flags = parse_cli_flags(&args);
        
        assert!(flags.print_mode);
        assert!(!flags.continue_session);
    }
    
    #[test]
    fn test_parse_continue_and_resume() {
        let args = vec![
            "carpai".to_string(),
            "-c".to_string(),
            "query".to_string()
        ];
        let flags = parse_cli_flags(&args);
        
        assert!(flags.continue_session);
    }
    
    #[test]
    fn test_parse_resume_with_name() {
        let args = vec![
            "carpai".to_string(),
            "-r".to_string(),
            "auth-refactor".to_string(),
            "query".to_string()
        ];
        let flags = parse_cli_flags(&args);
        
        assert_eq!(flags.resume_session, Some("auth-refactor".to_string()));
    }
    
    #[test]
    fn test_parse_multiple_add_dirs() {
        let args = vec![
            "carpai".to_string(),
            "--add-dir".to_string(),
            "../api".to_string(),
            "../lib".to_string(),
            "../utils".to_string()
        ];
        let flags = parse_cli_flags(&args);
        
        assert_eq!(flags.additional_dirs.len(), 3);
        assert!(flags.additional_dirs.contains(&"../api".to_string()));
    }
    
    #[test]
    fn test_parse_allowed_tools() {
        let args = vec![
            "carpai".to_string(),
            "--allowedTools".to_string(),
            r#""Bash(git log *)" "Bash(git diff *)" "Read""#.to_string()
        ];
        let flags = parse_cli_flags(&args);
        
        assert_eq!(flags.allowed_tools.len(), 3);
        assert!(flags.allowed_tools.contains(&"Bash(git log *)".to_string()));
    }
    
    #[test]
    fn test_parse_debug_with_categories() {
        let args = vec![
            "carpai".to_string(),
            "--debug".to_string(),
            "api,mcp".to_string()
        ];
        let flags = parse_cli_flags(&args);
        
        assert!(!flags.debug_mode); // 有具体类别时不是全局启用
        assert!(flags.is_debug_category("api"));
        assert!(flags.is_debug_category("mcp"));
        assert!(!flags.is_debug_category("statsig"));
    }
}
