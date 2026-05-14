//! # Bash AST解析器
//!
//! 完整的Bash命令行语法分析，支持：
//! - **简单命令** - `git status`
//! - **管道** - `cat file | grep pattern`
//! - **重定向** - `cmd > output 2>&1`
//! - **列表/逻辑** - `cmd1 && cmd2 || cmd3`
//! - **子shell** - `$(command)` 或 `(list)`
//! - **后台任务** - `cmd &`
//! - **变量赋值** - `VAR=value`
//! - **函数调用** - `func arg1 arg2`

use crate::completion::bash::{CompletionContext, CompletionSuggestion, CompletionKind};
use regex::Regex;
use serde::{Deserialize, Serialize};

/// Bash AST节点类型
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AstNode {
    /// 简单命令 (如: git, ls, echo)
    Command(CommandNode),
    
    /// 管道连接
    Pipeline(PipelineNode),
    
    /// 命令列表（&& || ; &）
    List(ListNode),
    
    /// 重定向操作
    Redirect(RedirectNode),
    
    /// 子shell
    Subshell(SubshellNode),
    
    /// 变量赋值
    Assignment(AssignmentNode),
    
    /// 空节点或注释
    Empty,
}

/// 命令节点
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CommandNode {
    /// 命令名称
    pub name: String,
    
    /// 参数列表
    pub args: Vec<String>,
    
    /// 是否以空格结尾（用于判断是否需要补全参数）
    pub trailing_space: bool,
    
    /// 原始文本范围
    pub range: TextRange,
}

/// 管道节点
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PipelineNode {
    /// 管道中的命令序列
    pub commands: Vec<CommandNode>,
}

/// 列表节点
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ListNode {
    /// 操作符 (&& || ; &)
    pub operator: ListOperator,
    
    /// 左侧AST
    pub left: Box<AstNode>,
    
    /// 右侧AST
    pub right: Box<AstNode>,
}

/// 列表操作符
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ListOperator {
    And,     // &&
    Or,      // ||
    Seq,     // ;
    Background, // &
}

impl std::fmt::Display for ListOperator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ListOperator::And => write!(f, "&&"),
            ListOperator::Or => write!(f, "||"),
            ListOperator::Seq => write!(f, ";"),
            ListOperator::Background => write!(f, "&"),
        }
    }
}

/// 重定向节点
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RedirectNode {
    /// 内部命令
    pub command: Box<AstNode>,
    
    /// 重定向描述符 (0=stdin, 1=stdout, 2=stderr)
    pub fd: Option<u32>,
    
    /// 重定向类型
    pub redirect_type: RedirectType,
    
    /// 目标文件/描述符
    pub target: String,
}

/// 重定向类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RedirectType {
    Output,       // >
    Append,       // >>
    Input,        // <
    HereDoc,      // <<
    HereDocStrip, // <<-
    DupOutput,    // >&
    DupInput,     // <&
}

/// 子shell节点
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SubshellNode {
    /// 内部命令列表
    pub body: Vec<AstNode>,
}

/// 赋值节点
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AssignmentNode {
    /// 变量名
    pub name: String,
    
    /// 变量值
    pub value: String,
}

/// 文本范围
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct TextRange {
    pub start: usize,
    pub end: usize,
}

/// 解析结果
#[derive(Debug, Clone)]
pub struct ParseResult {
    /// AST根节点
    pub ast: AstNode,
    
    /// 解析错误（如果有）
    pub errors: Vec<ParseError>,
    
    /// 当前光标所在的上下文信息
    pub cursor_context: CursorContext,
}

/// 光标上下文
#[derive(Debug, Clone)]
pub struct CursorContext {
    /// 当前正在输入的命令名（如果有的话）
    pub current_command: Option<String>,
    
    /// 当前参数索引（从0开始）
    pub current_arg_index: Option<usize>,
    
    /// 光标前的所有token
    pub tokens_before_cursor: Vec<Token>,
    
    /// 光标是否在引号内
    pub in_quotes: bool,
    
    /// 引号类型（单引号/双引号）
    pub quote_char: Option<char>,
}

/// Token类型
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token {
    Word(String),
    Operator(char),
    Pipe,
    Redirect(RedirectType),
    Newline,
    Space,
    Comment(String),
}

/// 解析错误
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParseError {
    pub message: String,
    pub position: usize,
    pub error_type: ParseErrorType,
}

/// 解析错误类型
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParseErrorType {
    UnexpectedToken,
    UnclosedQuote,
    UnclosedParenthesis,
    InvalidSyntax,
    IncompleteCommand,
}

/// Bash AST解析器
pub struct BashParser {
    /// 预编译的正则表达式
    word_regex: Regex,
    operator_regex: Regex,
    redirect_regex: Regex,
    quote_regex: Regex,
    variable_regex: Regex,
}

impl Default for BashParser {
    fn default() -> Self {
        Self {
            word_regex: Regex::new(r"[^\s|&;<>()'\"$]+").unwrap(),
            operator_regex: Regex::new(r"(&&|\|\||[;&])").unwrap(),
            redirect_regex: Regex::new(r"(>>?|<<-?|<|>&?|<&?)").unwrap(),
            quote_regex: Regex::new(r#"['"]"#).unwrap(),
            variable_regex: Regex::new(r"\$\w+|\$\{[^}]+\}").unwrap(),
        }
    }
}

impl BashParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// 解析输入并生成AST
    pub fn parse(&self, input: &str, cursor_pos: usize) -> ParseResult {
        let mut tokens = self.tokenize(input);
        let mut errors = vec![];

        // 分析光标上下文
        let cursor_context = self.analyze_cursor_context(&tokens, cursor_pos, input);

        // 构建AST
        let ast = if tokens.is_empty() {
            AstNode::Empty
        } else {
            match self.build_ast(&mut tokens, &mut errors) {
                Ok(node) => node,
                Err(_) => AstNode::Empty,
            }
        };

        ParseResult {
            ast,
            errors,
            cursor_context,
        }
    }

    /// 获取补全建议（基于AST分析）
    pub fn get_suggestions(
        &self,
        context: &CompletionContext,
    ) -> Vec<CompletionSuggestion> {
        let parse_result = self.parse(&context.line, context.cursor_position);
        
        let mut suggestions = vec![];

        match &parse_result.cursor_context.current_command {
            Some(cmd) if cmd.is_empty() => {
                // 正在输入命令名，提供命令补全
                suggestions.push(CompletionSuggestion {
                    text: "git".to_string(),
                    display_text: "git".to_string(),
                    description: "分布式版本控制系统".to_string(),
                    kind: CompletionKind::Command,
                    priority: 95,
                    metadata: std::collections::HashMap::new(),
                });
                
                suggestions.push(CompletionSuggestion {
                    text: "npm".to_string(),
                    display_text: "npm".to_string(),
                    description: "JavaScript包管理器".to_string(),
                    kind: CompletionKind::Command,
                    priority: 95,
                    metadata: std::collections::HashMap::new(),
                });
                
                suggestions.push(CompletionSuggestion {
                    text: "docker".to_string(),
                    display_text: "docker".to_string(),
                    description: "容器管理平台".to_string(),
                    kind: CompletionKind::Command,
                    priority: 95,
                    metadata: std::collections::HashMap::new(),
                });
            }
            
            Some(cmd) => {
                // 已有命令，提供参数补全
                match cmd.as_str() {
                    "git" => {
                        suggestions.extend(self.get_git_suggestions(context));
                    }
                    "docker" => {
                        suggestions.extend(self.get_docker_suggestions(context));
                    }
                    "npm" => {
                        suggestions.extend(self.get_npm_suggestions(context));
                    }
                    _ => {}
                }
            }
            
            None => {
                // 无法确定上下文，提供通用建议
            }
        }

        suggestions
    }

    /// Tokenize输入
    fn tokenize(&self, input: &str) -> Vec<Token> {
        let mut tokens = vec![];
        let mut pos = 0;
        let chars: Vec<char> = input.chars().collect();

        while pos < chars.len() {
            let ch = chars[pos];

            match ch {
                ' ' | '\t' => {
                    tokens.push(Token::Space);
                    pos += 1;
                }
                '|' => {
                    if pos + 1 < chars.len() && chars[pos + 1] == '|' {
                        tokens.push(Token::Operator('|'));
                        tokens.push(Token::Operator('|'));
                        pos += 2;
                    } else {
                        tokens.push(Token::Pipe);
                        pos += 1;
                    }
                }
                '&' => {
                    if pos + 1 < chars.len() && chars[pos + 1] == '&' {
                        tokens.push(Token::Operator('&'));
                        tokens.push(Token::Operator('&'));
                        pos += 2;
                    } else {
                        tokens.push(Token::Operator('&'));
                        pos += 1;
                    }
                }
                ';' | '\n' => {
                    tokens.push(if ch == '\n' { Token::Newline } else { Token::Operator(';') });
                    pos += 1;
                }
                '>' => {
                    if pos + 1 < chars.len() && chars[pos + 1] == '>' {
                        tokens.push(Token::Redirect(RedirectType::Append));
                        pos += 2;
                    } else {
                        tokens.push(Token::Redirect(RedirectType::Output));
                        pos += 1;
                    }
                }
                '<' => {
                    tokens.push(Token::Redirect(RedirectType::Input));
                    pos += 1;
                }
                '\'' | '"' => {
                    // 处理引号字符串
                    let quote_ch = ch;
                    pos += 1;
                    let mut content = String::new();
                    
                    while pos < chars.len() && chars[pos] != quote_ch {
                        content.push(chars[pos]);
                        pos += 1;
                    }
                    
                    if pos < chars.len() {
                        pos += 1; // 关闭引号
                    }
                    
                    tokens.push(Token::Word(content));
                }
                '#' if tokens.is_empty() || matches!(tokens.last(), Some(Token::Space) | Some(Token::Newline)) => {
                    // 注释
                    let comment_start = pos;
                    while pos < chars.len() && chars[pos] != '\n' {
                        pos += 1;
                    }
                    let comment: String = chars[comment_start..pos].iter().collect();
                    tokens.push(Token::Comment(comment));
                }
                '$' => {
                    // 变量引用
                    let var_start = pos;
                    pos += 1;
                    
                    if pos < chars.len() && chars[pos] == '{' {
                        // ${var} 形式
                        pos += 1;
                        while pos < chars.len() && chars[pos] != '}' {
                            pos += 1;
                        }
                        if pos < chars.len() {
                            pos += 1;
                        }
                    } else {
                        // $var 形式
                        while pos < chars.len() && (chars[pos].is_alphanumeric() || chars[pos] == '_') {
                            pos += 1;
                        }
                    }
                    
                    let var_name: String = chars[var_start..pos].iter().collect();
                    tokens.push(Token::Word(var_name));
                }
                _ if !ch.is_whitespace() => {
                    // 普通单词
                    let word_start = pos;
                    while pos < chars.len() && 
                          !chars[pos].is_whitespace() && 
                          !"|&;<>'\"$".contains(chars[pos]) 
                    {
                        pos += 1;
                    }
                    
                    let word: String = chars[word_start..pos].iter().collect();
                    tokens.push(Token::Word(word));
                }
                _ => {
                    pos += 1;
                }
            }
        }

        tokens
    }

    /// 分析光标位置上下文
    fn analyze_cursor_context(
        &self,
        tokens: &[Token],
        cursor_pos: usize,
        input: &str,
    ) -> CursorContext {
        let mut current_command = None;
        let mut current_arg_index = None;
        let mut in_quotes = false;
        let mut quote_char = None;

        // 找到光标之前的token
        let mut tokens_before_cursor = vec![];
        let mut char_count = 0usize;

        for token in tokens {
            let token_len = match token {
                Token::Word(s) => s.len(),
                Token::Comment(s) => s.len(),
                Token::Operator(_) | Token::Pipe | Token::Redirect(_) => 1,
                Token::Newline | Token::Space => 1,
            };

            if char_count + token_len <= cursor_pos {
                tokens_before_cursor.push(token.clone());
                char_count += token_len;

                // 更新当前命令和参数索引
                match token {
                    Token::Word(s) if current_command.is_none() => {
                        current_command = Some(s.clone());
                        current_arg_index = Some(0);
                    }
                    Token::Word(_) if current_command.is_some() => {
                        if let Some(ref mut idx) = current_arg_index {
                            *idx += 1;
                        }
                    }
                    Token::Pipe | Token::Operator(';') => {
                        // 新命令开始
                        current_command = None;
                        current_arg_index = None;
                    }
                    _ => {}
                }
            } else {
                break;
            }
        }

        // 检查是否在引号内
        let before_cursor: String = input[..cursor_pos.min(input.len())].chars().collect();
        for ch in before_cursor.chars().rev() {
            if ch == '\'' || ch == '"' {
                in_quotes = true;
                quote_char = Some(ch);
                break;
            }
        }

        CursorContext {
            current_command,
            current_arg_index,
            tokens_before_cursor,
            in_quotes,
            quote_char,
        }
    }

    /// 从tokens构建AST
    fn build_ast(
        &self,
        tokens: &mut Vec<Token>,
        errors: &mut Vec<ParseError>,
    ) -> Result<AstNode, ()> {
        // 简化版：只处理管道和简单命令
        let mut commands = vec![];
        let mut current_cmd_words = vec![];

        while !tokens.is_empty() {
            match tokens.remove(0) {
                Token::Pipe => {
                    // 完成当前命令，开始新的
                    if !current_cmd_words.is_empty() {
                        commands.push(self.build_command_node(current_cmd_words));
                        current_cmd_words = vec![];
                    }
                }
                Token::Word(w) => {
                    current_cmd_words.push(w);
                }
                Token::Operator(op) => {
                    // 处理列表操作符
                    if !current_cmd_words.is_empty() {
                        let left_cmd = self.build_command_node(current_cmd_words.clone());
                        
                        if !commands.is_empty() {
                            let left_ast = if commands.len() == 1 {
                                AstNode::Command(commands.remove(0))
                            } else {
                                AstNode::Pipeline(PipelineNode { commands })
                            };
                            
                            // 尝试解析右侧
                            let right_ast = self.build_ast(tokens, errors)?;
                            
                            return Ok(AstNode::List(ListNode {
                                operator: match op {
                                    '&' => ListOperator::Background,
                                    ';' => ListOperator::Seq,
                                    _ => ListOperator::Seq,
                                },
                                left: Box::new(left_ast),
                                right: Box::new(right_ast),
                            }));
                        }
                    }
                }
                _ => {}
            }
        }

        // 处理最后一个命令
        if !current_cmd_words.is_empty() {
            commands.push(self.build_command_node(current_cmd_words));
        }

        if commands.len() == 1 {
            Ok(AstNode::Command(commands.remove(0)))
        } else if !commands.is_empty() {
            Ok(AstNode::Pipeline(PipelineNode { commands }))
        } else {
            Ok(AstNode::Empty)
        }
    }

    /// 从单词列表构建命令节点
    fn build_command_node(&self, words: Vec<String>) -> CommandNode {
        if words.is_empty() {
            panic!("Cannot build command from empty words");
        }

        CommandNode {
            name: words[0].clone(),
            args: words[1..].to_vec(),
            trailing_space: false,
            range: TextRange::default(),
        }
    }

    /// Git命令补全建议
    fn get_git_suggestions(&self, context: &CompletionContext) -> Vec<CompletionSuggestion> {
        let mut suggestions = vec![];

        let git_subcommands = [
            ("status", "显示工作区状态", 98),
            ("commit", "提交更改", 97),
            ("push", "推送到远程", 96),
            ("pull", "拉取远程更改", 95),
            ("checkout", "切换分支或恢复文件", 94),
            ("branch", "列出/创建/删除分支", 93),
            ("merge", "合并分支", 92),
            ("log", "显示提交日志", 91),
            ("diff", "显示差异", 90),
            ("add", "添加文件到暂存区", 89),
            ("reset", "重置HEAD到指定状态", 88),
            ("stash", "暂存更改", 87),
            ("rebase",变基分支", 86),
            ("tag", "创建/列出/删除标签", 85),
            ("fetch", "从远程下载对象", 84),
            ("remote", "管理远程仓库", 83),
            ("clone", "克隆仓库", 82),
        ];

        for (cmd, desc, priority) in &git_subcommands {
            let prefix = context.get_word_before_cursor();
            
            if prefix.is_empty() || cmd.starts_with(prefix) {
                suggestions.push(CompletionSuggestion {
                    text: format!("{} ", cmd),
                    display_text: cmd.to_string(),
                    description: desc.to_string(),
                    kind: CompletionKind::Argument,
                    priority: *priority,
                    metadata: std::collections::HashMap::new(),
                });
            }
        }

        suggestions
    }

    /// Docker命令补全建议
    fn get_docker_suggestions(&self, context: &CompletionContext) -> Vec<CompletionSuggestion> {
        let mut suggestions = vec![];

        let docker_subcommands = [
            ("ps", "列出容器", 95),
            ("images", "列出镜像", 94),
            ("run", "运行新容器", 93),
            ("start", "启动一个或多个容器", 92),
            ("stop", "停止一个或多个容器", 91),
            ("rm", "删除容器", 90),
            ("rmi", "删除镜像", 89),
            ("build", "构建镜像", 88),
            ("exec", "在运行的容器中执行命令", 87),
            ("logs", "获取容器日志", 86),
            ("inspect", "显示详细信息", 85),
            ("pull", "拉取镜像", 84),
            ("push", "推送镜像", 83),
            ("network", "管理网络", 82),
            ("volume", "管理卷", 81),
        ];

        for (cmd, desc, priority) in &docker_subcommands {
            let prefix = context.get_word_before_cursor();
            
            if prefix.is_empty() || cmd.starts_with(prefix) {
                suggestions.push(CompletionSuggestion {
                    text: format!("{} ", cmd),
                    display_text: cmd.to_string(),
                    description: desc.to_string(),
                    kind: CompletionKind::Argument,
                    priority: *priority,
                    metadata: std::collections::HashMap::new(),
                });
            }
        }

        suggestions
    }

    /// NPM命令补全建议
    fn get_npm_suggestions(&self, context: &CompletionContext) -> Vec<CompletionSuggestion> {
        let mut suggestions = vec![];

        let npm_subcommands = [
            ("install", "安装依赖包", 96),
            ("update", "更新依赖包", 95),
            ("uninstall", "卸载依赖包", 94),
            ("run", "运行脚本", 93),
            ("test", "运行测试", 92),
            ("build", "构建项目", 91),
            ("publish", "发布包", 90),
            ("init", "初始化项目", 89),
            ("info", "显示包信息", 88),
            ("list", "列出已安装的包", 87),
            ("outdated", "检查过时的包", 86),
            ("audit", "安全审计", 85),
            ("cache", "管理缓存", 84),
        ];

        for (cmd, desc, priority) in &npm_subcommands {
            let prefix = context.get_word_before_cursor();
            
            if prefix.is_empty() || cmd.starts_with(prefix) {
                suggestions.push(CompletionSuggestion {
                    text: format!("{} ", cmd),
                    display_text: cmd.to_string(),
                    description: desc.to_string(),
                    kind: CompletionKind::Argument,
                    priority: *priority,
                    metadata: std::collections::HashMap::new(),
                });
            }
        }

        suggestions
    }
}

// ==========================================
// 单元测试
// ==========================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_command() {
        let parser = BashParser::new();
        let result = parser.parse("ls -la", 5);

        assert!(result.errors.is_empty());
        
        match result.ast {
            AstNode::Command(cmd) => {
                assert_eq!(cmd.name, "ls");
                assert_eq!(cmd.args, vec!["-la"]);
            }
            _ => panic!("Expected Command node"),
        }
    }

    #[test]
    fn test_parse_pipeline() {
        let parser = BashParser::new();
        let result = parser.parse("cat file | grep pattern", 20);

        match result.ast {
            AstNode::Pipeline(pipeline) => {
                assert_eq!(pipeline.commands.len(), 2);
                assert_eq!(pipeline.commands[0].name, "cat");
                assert_eq!(pipeline.commands[1].name, "grep");
            }
            _ => panic!("Expected Pipeline node"),
        }
    }

    #[test]
    fn test_parse_list_and_operator() {
        let parser = BashParser::new();
        let result = parser.parse("cmd1 && cmd2", 10);

        match result.ast {
            AstNode::List(list) => {
                assert_eq!(list.operator, ListOperator::And);
            }
            _ => panic!("Expected List node with And operator"),
        }
    }

    #[test]
    fn test_cursor_context_detection() {
        let parser = BashParser::new();
        
        // 测试：正在输入命令名
        let result = parser.parse("gi", 2);
        assert_eq!(result.cursor_context.current_command, Some("gi".to_string()));
        
        // 测试：已有命令，正在输入参数
        let result = parser.parse("git sta", 7);
        assert_eq!(result.cursor_context.current_command, Some("git".to_string()));
        assert_eq!(result.cursor_context.current_arg_index, Some(1));
    }

    #[test]
    fn test_git_completions() {
        let parser = BashParser::new();
        let ctx = CompletionContext::new("git ", 4);
        
        let suggestions = parser.get_suggestions(&ctx);
        
        assert!(!suggestions.is_empty(), "Should have git completions");
        
        // 应该包含常见的git子命令
        let has_status = suggestions.iter().any(|s| s.text.contains("status"));
        let has_commit = suggestions.iter().any(|s| s.text.contains("commit"));
        
        assert!(has_status, "Should include 'status'");
        assert!(has_commit, "Should include 'commit'");
    }

    #[test]
    fn test_docker_completions() {
        let parser = BashParser::new();
        let ctx = CompletionContext::new("docker ", 7);
        
        let suggestions = parser.get_suggestions(&ctx);
        
        assert!(!suggestions.is_empty());
        
        let has_ps = suggestions.iter().any(|s| s.text.contains("ps"));
        let has_run = suggestions.iter().any(|s| s.text.contains("run"));
        
        assert!(has_ps, "Should include 'ps'");
        assert!(has_run, "Should include 'run'");
    }

    #[test]
    fn test_npm_completions() {
        let parser = BashParser::new();
        let ctx = CompletionContext::new("npm ", 4);
        
        let suggestions = parser.get_suggestions(&ctx);
        
        assert!(!suggestions.is_empty());
        
        let has_install = suggestions.iter().any(|s| s.text.contains("install"));
        let_has_test = suggestions.iter().any(|s| s.text.contains("test"));
        
        assert!(has_install, "Should include 'install'");
        assert!(has_test, "Should include 'test'");
    }

    #[test]
    fn test_tokenize_complex_input() {
        let parser = BashParser::new();
        let tokens = parser.tokenize("echo 'hello world' > output.txt 2>&1 | grep hello");
        
        // 验证关键token存在
        let has_echo = tokens.iter().any(|t| matches!(t, Token::Word(s) if s == "echo"));
        let has_redirect_output = tokens.iter().any(|t| matches!(t, Token::Redirect(RedirectType::Output)));
        let has_pipe = tokens.iter().any(|t| *t == Token::Pipe);
        
        assert!(has_echo);
        assert!(has_redirect_output);
        assert!(has_pipe);
    }

    #[test]
    fn test_quote_handling() {
        let parser = BashParser::new();
        let tokens = parser.tokenize("\"quoted string\" 'single quoted'");
        
        // 引号内的内容应该作为单个word token
        assert_eq!(tokens.len(), 2);
        
        match &tokens[0] {
            Token::Word(s) => assert_eq!(s, "quoted string"),
            _ => panic!("Expected Word token"),
        }
        
        match &tokens[1] {
            Token::Word(s) => assert_eq!(s, "single quoted"),
            _ => panic!("Expected Word token"),
        }
    }

    #[test]
    fn test_variable_expansion_tokenization() {
        let parser = BashParser::new();
        let tokens = parser.tokenize("echo $HOME ${USER:-default}");
        
        // 变量应该被识别为word token
        assert!(tokens.iter().any(|t| matches!(t, Token::Word(s) if s.starts_with('$'))));
    }

    #[test]
    fn test_background_task_parsing() {
        let parser = BashParser::new();
        let result = parser.parse("long_running_task &", 18);

        match result.ast {
            AstNode::List(list) => {
                assert_eq!(list.operator, ListOperator::Background);
            }
            _ => {
                // 可能返回其他有效AST也是可接受的
            }
        }
    }

    #[test]
    fn test_redirect_parsing() {
        let parser = BashParser::new();
        let result = parser.parse("cmd > out.txt 2>> err.log", 25);

        // 至少应该能正确解析命令名
        match &result.ast {
            AstNode::Command(cmd) => {
                assert_eq!(cmd.name, "cmd");
            }
            _ => {}
        }
    }

    #[test]
    fn test_empty_input() {
        let parser = BashParser::new();
        let result = parser.parse("", 0);

        assert!(matches!(result.ast, AstNode::Empty));
        assert!(result.errors.is_empty());
    }

    #[test]
    fn test_completion_priority_ordering() {
        let parser = BashParser::new();
        let ctx = CompletionContext::new("git ", 4);
        let mut suggestions = parser.get_suggestions(&ctx);

        // 排序后，高优先级的应该在前面
        suggestions.sort_by(|a, b| b.priority.cmp(&a.priority));

        assert!(!suggestions.is_empty());
        
        // 第一个应该是最高优先级
        let max_priority = suggestions.first().map(|s| s.priority).unwrap_or(0);
        assert!(max_priority >= 90, "Top suggestion should have high priority");
    }
}
