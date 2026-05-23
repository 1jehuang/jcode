//! # 工具提示词系统 — 借鉴 Claude Code 每工具独立 prompt.ts 架构
//!
//! 每个工具实现 `ToolPrompt` trait，提供其在 system prompt 中的描述，
//! 包括使用方式、参数说明、注意事项和示例。
//!
//! 工具提示词由 `PromptBuilder::build_tool_prompts()` 统一组装成
//! system prompt 的一部分，供 LLM 理解每个工具的正确用法。

use std::collections::BTreeMap;

/// 工具提示词 — 描述工具的正确用法
#[derive(Debug, Clone)]
pub struct ToolPrompt {
    /// 工具名
    pub name: &'static str,
    /// 工具用途简短描述
    pub purpose: &'static str,
    /// 使用方式详细说明（多行）
    pub usage: &'static str,
    /// 参数说明
    pub parameters: Vec<ParamDesc>,
    /// 使用注意事项
    pub notes: Vec<&'static str>,
    /// 使用示例
    pub examples: Vec<UsageExample>,
    /// 是否只读
    pub is_read_only: bool,
}

/// 参数描述
#[derive(Debug, Clone)]
pub struct ParamDesc {
    pub name: &'static str,
    pub param_type: &'static str,
    pub required: bool,
    pub description: &'static str,
}

/// 使用示例
#[derive(Debug, Clone)]
pub struct UsageExample {
    pub description: &'static str,
    pub input: &'static str,
    pub output: &'static str,
}

/// 提示词构建器 — 将所有工具的提示词组装成 system prompt 片段
pub struct PromptBuilder {
    tool_prompts: BTreeMap<&'static str, ToolPrompt>,
}

impl PromptBuilder {
    pub fn new() -> Self {
        Self {
            tool_prompts: BTreeMap::new(),
        }
    }

    /// 注册一个工具的提示词
    pub fn register(&mut self, prompt: ToolPrompt) {
        self.tool_prompts.insert(prompt.name, prompt);
    }

    /// 构建所有工具的提示词文本（Markdown 格式）
    pub fn build_tool_prompts(&self, tool_names: &[&str]) -> String {
        let mut output = String::new();
        output.push_str("## 可用工具\n\n");

        for name in tool_names {
            if let Some(prompt) = self.tool_prompts.get(name) {
                output.push_str(&self.format_tool_prompt(prompt));
            }
        }

        output
    }

    /// 构建单个工具的提示词文本
    fn format_tool_prompt(&self, prompt: &ToolPrompt) -> String {
        let mut s = String::new();

        // 工具名 + 用途
        s.push_str(&format!("### {}\n\n", prompt.name));
        s.push_str(&format!("**用途**: {}\n\n", prompt.purpose));

        // 使用方式
        if !prompt.usage.is_empty() {
            s.push_str(&format!("**使用方式**:\n{}\n\n", prompt.usage));
        }

        // 参数
        if !prompt.parameters.is_empty() {
            s.push_str("**参数**:\n");
            for p in &prompt.parameters {
                let req = if p.required { "必填" } else { "可选" };
                s.push_str(&format!(
                    "- `{}` ({}, {}): {}\n",
                    p.name, p.param_type, req, p.description
                ));
            }
            s.push('\n');
        }

        // 注意事项
        if !prompt.notes.is_empty() {
            s.push_str("**注意事项**:\n");
            for note in &prompt.notes {
                s.push_str(&format!("- {}\n", note));
            }
            s.push('\n');
        }

        // 示例
        if !prompt.examples.is_empty() {
            s.push_str("**示例**:\n");
            for ex in &prompt.examples {
                s.push_str(&format!("- *{}*\n  ```json\n  {}\n  ```\n", ex.description, ex.input));
            }
            s.push('\n');
        }

        s.push_str("---\n\n");
        s
    }
}

// === 内置工具提示词定义 ===

/// EditTool 提示词
pub fn edit_tool_prompt() -> ToolPrompt {
    ToolPrompt {
        name: "edit",
        purpose: "对文件进行精确的文本替换编辑。当需要修改文件中特定内容时使用。",
        usage: "提供 `file_path`、`old_string`（要替换的原文）和 `new_string`（替换后的内容）。\
               \n\n匹配策略（从 Claude Code 借鉴）:\n\
               \n1. **精确匹配**: 首先尝试完全匹配 old_string\n\
               \n2. **模糊匹配**: 如果精确匹配失败，尝试去除两端空白后匹配\n\
               \n3. **行尾归一化**: 忽略 CRLF/LF 差异进行匹配\n\
               \n4. **唯一性验证**: 确保匹配结果唯一，避免误替换",
        parameters: vec![
            ParamDesc { name: "file_path", param_type: "string", required: true, description: "目标文件路径" },
            ParamDesc { name: "old_string", param_type: "string", required: true, description: "要替换的原文（推荐包含前后文以保证唯一匹配）" },
            ParamDesc { name: "new_string", param_type: "string", required: true, description: "替换后的内容" },
            ParamDesc { name: "replace_all", param_type: "boolean", required: false, description: "是否替换所有匹配项（默认仅替换第一个）" },
        ],
        notes: vec![
            "始终包含足够的上下文以确保 old_string 在文件中唯一匹配",
            "如果替换后文件格式异常，使用 write 工具重新写入整个文件",
            "对大型文件（>1MB）建议使用分段编辑，避免单次替换过多内容",
        ],
        examples: vec![
            UsageExample {
                description: "替换函数名",
                input: r#"{"file_path": "src/main.rs", "old_string": "fn old_name()", "new_string": "fn new_name()"}"#,
                output: "文件中的 `fn old_name()` 被替换为 `fn new_name()`",
            },
        ],
        is_read_only: false,
    }
}

/// ReadTool 提示词
pub fn read_tool_prompt() -> ToolPrompt {
    ToolPrompt {
        name: "read",
        purpose: "读取文件内容。当需要查看代码、配置文件或任何文件内容时使用。",
        usage: "提供 `file_path` 即可读取文件内容。可选的 `offset` 和 `limit` 参数用于读取文件的部分内容。",
        parameters: vec![
            ParamDesc { name: "file_path", param_type: "string", required: true, description: "文件路径" },
            ParamDesc { name: "offset", param_type: "integer", required: false, description: "读取起始行号（从 0 开始）" },
            ParamDesc { name: "limit", param_type: "integer", required: false, description: "读取行数上限" },
        ],
        notes: vec!["对于大文件，建议使用 offset 和 limit 分段读取"],
        examples: vec![
            UsageExample {
                description: "读取文件前 50 行",
                input: r#"{"file_path": "src/main.rs", "limit": 50}"#,
                output: "文件内容（前 50 行）",
            },
        ],
        is_read_only: true,
    }
}

/// BashTool 提示词
pub fn bash_tool_prompt() -> ToolPrompt {
    ToolPrompt {
        name: "bash",
        purpose: "执行 shell 命令。用于运行脚本、编译代码、启动服务等。",
        usage: "提供 `command` 字符串执行。\
               \n\n安全策略:\
               \n- 命令超时默认 120 秒\
               \n- 输出截断至 30000 字符\
               \n- 交互式命令需通过 `stdin` 输入\
               \n- 长时间运行的任务建议使用 `background` 模式",
        parameters: vec![
            ParamDesc { name: "command", param_type: "string", required: true, description: "要执行的 shell 命令" },
            ParamDesc { name: "description", param_type: "string", required: false, description: "命令用途描述" },
            ParamDesc { name: "timeout", param_type: "integer", required: false, description: "超时时间（毫秒，默认 120000）" },
            ParamDesc { name: "is_background", param_type: "boolean", required: false, description: "是否后台运行" },
            ParamDesc { name: "stdin", param_type: "string", required: false, description: "标准输入内容" },
        ],
        notes: vec![
            "优先使用非交互式命令以避免进程挂起",
            "长时间运行的任务请在命令中输出 JCODE_PROGRESS 格式的进度信息",
            "安装依赖或编译可能较慢，使用 --progress 风格确认进度",
        ],
        examples: vec![
            UsageExample {
                description: "运行测试",
                input: r#"{"command": "cargo test", "description": "运行单元测试", "timeout": 300000}"#,
                output: "测试结果输出",
            },
            UsageExample {
                description: "后台运行编译",
                input: r#"{"command": "cargo build --release", "is_background": true}"#,
                output: "后台任务 ID",
            },
        ],
        is_read_only: false,
    }
}

/// GlobTool 提示词
pub fn glob_tool_prompt() -> ToolPrompt {
    ToolPrompt {
        name: "glob",
        purpose: "查找匹配通配符模式的文件。用于发现项目中的文件。",
        usage: "提供 glob 模式（如 `**/*.rs`）查找匹配的文件列表。",
        parameters: vec![
            ParamDesc { name: "pattern", param_type: "string", required: true, description: "glob 搜索模式" },
            ParamDesc { name: "directory", param_type: "string", required: false, description: "搜索目录（默认当前工作目录）" },
        ],
        notes: vec![
            "使用 `**` 进行递归搜索",
            "搜索结果不会包含 .gitignore 中的文件",
        ],
        examples: vec![
            UsageExample {
                description: "查找所有 Rust 文件",
                input: r#"{"pattern": "**/*.rs"}"#,
                output: "匹配的文件列表",
            },
        ],
        is_read_only: true,
    }
}

/// GrepTool 提示词
pub fn grep_tool_prompt() -> ToolPrompt {
    ToolPrompt {
        name: "grep",
        purpose: "在项目中搜索文本内容。用于查找函数定义、变量引用等。",
        usage: "提供 `pattern` 正则表达式进行全文搜索。",
        parameters: vec![
            ParamDesc { name: "pattern", param_type: "string", required: true, description: "搜索模式（支持正则表达式）" },
            ParamDesc { name: "path", param_type: "string", required: false, description: "搜索路径" },
            ParamDesc { name: "glob", param_type: "string", required: false, description: "文件过滤模式" },
            ParamDesc { name: "context", param_type: "integer", required: false, description: "上下文行数" },
        ],
        notes: vec!["对于代码搜索，建议同时使用 glob 参数限定文件类型以提高精度"],
        examples: vec![
            UsageExample {
                description: "搜索函数定义",
                input: r#"{"pattern": "fn handle_request", "glob": "*.rs"}"#,
                output: "匹配的代码行",
            },
        ],
        is_read_only: true,
    }
}

/// WriteTool 提示词
pub fn write_tool_prompt() -> ToolPrompt {
    ToolPrompt {
        name: "write",
        purpose: "创建新文件或完全覆盖已有文件。",
        usage: "提供 `file_path` 和 `file_content` 来创建或覆盖文件。",
        parameters: vec![
            ParamDesc { name: "file_path", param_type: "string", required: true, description: "文件路径" },
            ParamDesc { name: "file_content", param_type: "string", required: true, description: "文件完整内容" },
        ],
        notes: vec![
            "此工具会完全覆盖文件内容，慎用",
            "对于修改现有文件，优先使用 edit 工具",
        ],
        examples: vec![
            UsageExample {
                description: "创建新文件",
                input: r#"{"file_path": "src/new_module.rs", "file_content": "pub fn hello() { println!(\"Hello\"); }"}"#,
                output: "文件创建成功",
            },
        ],
        is_read_only: false,
    }
}

/// WebFetchTool 提示词
pub fn webfetch_tool_prompt() -> ToolPrompt {
    ToolPrompt {
        name: "webfetch",
        purpose: "获取网页内容用于阅读和分析。",
        usage: "提供 URL 获取网页内容（转换为 Markdown）。",
        parameters: vec![
            ParamDesc { name: "url", param_type: "string", required: true, description: "目标 URL" },
        ],
        notes: vec!["仅用于获取公开可访问的网页内容"],
        examples: vec![
            UsageExample {
                description: "获取文档页面",
                input: r#"{"url": "https://docs.rs/tokio/latest/tokio/"}"#,
                output: "页面的 Markdown 格式内容",
            },
        ],
        is_read_only: true,
    }
}

/// WebSearchTool 提示词
pub fn websearch_tool_prompt() -> ToolPrompt {
    ToolPrompt {
        name: "websearch",
        purpose: "搜索互联网获取最新信息。当需要查找最新的 API 文档、技术方案或解决问题时使用。",
        usage: "提供搜索查询字符串。",
        parameters: vec![
            ParamDesc { name: "query", param_type: "string", required: true, description: "搜索查询" },
        ],
        notes: vec!["优先使用英文关键词以获取更准确的结果"],
        examples: vec![],
        is_read_only: true,
    }
}
