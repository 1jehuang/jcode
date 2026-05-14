//! # Bash补全引擎
//!
//! 提供智能的Bash命令行补全功能：
//! - **AST解析器** - 完整的Bash语法分析
//! - **命令注册表** - 50+内置命令规格
//! - **动态补全** - Git分支/Docker容器等实时数据
//! - **上下文感知** - 基于项目环境调整建议

pub mod parser;
pub mod registry;
pub mod specs;
pub mod completer;
pub mod snippet;

use serde::{Deserialize, Serialize};

/// 补全建议
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionSuggestion {
    /// 补全文本
    pub text: String,
    
    /// 显示文本（可能包含格式化）
    pub display_text: String,
    
    /// 详细描述
    pub description: String,
    
    /// 补全类型
    pub kind: CompletionKind,
    
    /// 优先级 (0-100, 越高越优先)
    pub priority: u8,
    
    /// 额外元数据
    pub metadata: std::collections::HashMap<String, String>,
}

/// 补全类型分类
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionKind {
    /// 命令名 (git, npm, docker)
    Command,
    /// 参数/选项 (--help, -v)
    Argument,
    /// 文件路径 (*.rs, *.ts)
    File,
    /// 目录路径 (./src/, /tmp/)
    Directory,
    /// 代码片段
    Snippet,
    /// Shell别名展开
    Alias,
    /// 环境变量 ($HOME, $PATH)
    Variable,
    /// Git分支/标签
    GitRef,
    /// Docker实体 (container/image)
    DockerEntity,
    /// 其他
    Other,
}

impl std::fmt::Display for CompletionKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompletionKind::Command => write!(f, "cmd"),
            CompletionKind::Argument => write!(f, "arg"),
            CompletionKind::File => write!(f, "file"),
            CompletionKind::Directory => write!(f, "dir"),
            CompletionKind::Snippet => write!(f, "snippet"),
            CompletionKind::Alias => write!(f, "alias"),
            CompletionKind::Variable => write!(f, "var"),
            CompletionKind::GitRef => write!(f, "git"),
            CompletionKind::DockerEntity => write!(f, "docker"),
            CompletionKind::Other => write!(f, "other"),
        }
    }
}

/// 补全请求上下文
#[derive(Debug, Clone)]
pub struct CompletionContext {
    /// 完整的输入行
    pub line: String,
    
    /// 光标位置
    pub cursor_position: usize,
    
    /// 当前工作目录
    pub working_directory: Option<std::path::PathBuf>,
    
    /// 项目根目录（如果可检测）
    pub project_root: Option<std::path::PathBuf>,
    
    /// 环境变量
    pub environment_vars: std::collections::HashMap<String, String>,
}

impl CompletionContext {
    pub fn new(line: &str, cursor_pos: usize) -> Self {
        Self {
            line: line.to_string(),
            cursor_position: cursor_pos,
            working_directory: None,
            project_root: None,
            environment_vars: std::env::vars().collect(),
        }
    }

    pub fn with_working_dir(mut self, path: &std::path::Path) -> Self {
        self.working_directory = Some(path.to_path_buf());
        self
    }

    pub fn with_project_root(mut self, path: &std::path::Path) -> Self {
        self.project_root = Some(path.to_path_buf());
        self
    }

    /// 获取光标前的单词
    pub fn get_word_before_cursor(&self) -> &str {
        if self.cursor_position == 0 || self.line.is_empty() {
            return "";
        }

        let end = self.cursor_position.min(self.line.len());
        
        // 找到单词开始位置（空格分隔）
        let start = self.line[..end]
            .rfind(|c: char| c.is_whitespace())
            .map_or(0, |pos| pos + 1);
        
        &self.line[start..end]
    }

    /// 获取光标前的完整命令（用于判断上下文）
    pub fn get_command_prefix(&self) -> &str {
        if self.cursor_position == 0 || self.line.is_empty() {
            return "";
        }

        let end = self.cursor_position.min(self.line.len());
        &self.line[..end]
    }
}

/// 补全结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionResult {
    /// 建议列表（按优先级排序）
    pub suggestions: Vec<CompletionSuggestion>,
    
    /// 要替换的起始位置
    pub replace_start: usize,
    
    /// 要替换的结束位置
    pub replace_end: usize,
    
    /// 是否有更多结果（分页）
    pub has_more: bool,
}

impl CompletionResult {
    /// 创建空结果
    pub fn empty(replace_start: usize, replace_end: usize) -> Self {
        Self {
            suggestions: vec![],
            replace_start,
            replace_end,
            has_more: false,
        }
    }

    /// 按优先级排序
    pub fn sort_by_priority(&mut self) {
        self.suggestions.sort_by(|a, b| b.priority.cmp(&a.priority));
    }

    /// 过滤结果
    pub fn filter<F>(&mut self, filter: F) where F: Fn(&CompletionSuggestion) -> bool {
        self.suggestions.retain(filter);
    }

    /// 限制结果数量
    pub fn limit(&mut self, max: usize) {
        if self.suggestions.len() > max {
            self.suggestions.truncate(max);
            self.has_more = true;
        }
    }
}
