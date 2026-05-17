//! # PowerShell 补全引擎
//!
//! 提供智能的 PowerShell 命令行补全功能：
//! - **Cmdlet 解析器** - 完整的 PowerShell 语法分析 (Verb-Noun)
//! - **参数补全** - (-Name, -Path, -Force 等)
//! - **管道语法** - (| Where-Object, | ForEach-Object)
//! - **变量支持** - ($env:, $HOME, $PSVersionTable)
//! - **内置 Cmdlet 注册表** - 50+ 常用命令
//! - **模块感知** - Import-Module 检测

use crate::completion::bash::{CompletionContext, CompletionSuggestion, CompletionKind};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// PowerShell AST 节点类型
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PsAstNode {
    /// Cmdlet 命令 (Get-ChildItem, Write-Host)
    Cmdlet(PsCmdletNode),
    /// 管道连接
    Pipeline(PsPipelineNode),
    /// 命令列表 (; 分隔)
    StatementList(Vec<PsAstNode>),
    /// 变量赋值 ($x = value)
    Assignment(PsAssignmentNode),
    /// 子表达式 $(...)
    SubExpression(PsSubExprNode),
    /// 空节点或注释
    Empty,
}

/// Cmdlet 节点
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PsCmdletNode {
    /// Cmdlet 名称 (Verb-Noun 格式)
    pub name: String,
    
    /// 参数列表
    pub parameters: Vec<PsParameter>,
    
    /// 是否以空格结尾
    pub trailing_space: bool,
    
    /// 原始文本范围
    pub range: PsTextRange,
}

/// PowerShell 参数
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PsParameter {
    /// 参数名 (-Path, -Name 等)
    pub name: Option<String>,
    
    /// 参数值
    pub value: Option<String>,
    
    /// 是否是开关参数 (如 -Force)
    pub is_switch: bool,
    
    /// 是否是位置参数（省略了参数名的参数）
    pub is_positional: bool,
}

/// 管道节点
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PsPipelineNode {
    pub commands: Vec<PsCmdletNode>,
}

/// 赋值节点
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PsAssignmentNode {
    pub variable_name: String,
    pub value_expr: String,
}

/// 子表达式节点
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PsSubExprNode {
    pub body: Vec<PsAstNode>,
}

/// 文本范围
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct PsTextRange {
    pub start: usize,
    pub end: usize,
}

/// 解析结果
#[derive(Debug, Clone)]
pub struct PsParseResult {
    pub ast: PsAstNode,
    pub errors: Vec<PsParseError>,
    pub cursor_context: PsCursorContext,
}

/// 光标上下文
#[derive(Debug, Clone)]
pub struct PsCursorContext {
    pub current_cmdlet: Option<String>,
    pub current_param_index: Option<usize>,
    pub in_pipeline: bool,
    pub pipeline_position: Option<usize>,
    pub tokens_before_cursor: Vec<PsToken>,
    pub in_variable_expansion: bool,
    pub in_string: bool,
    pub string_type: Option<char>,
}

/// Token 类型
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PsToken {
    Cmdlet(String),
    Parameter(String),
    ParameterValue(String),
    Pipe,
    Semicolon,
    Equals,
    Variable(String),
    DollarParenOpen,
    DollarParenClose,
    ParenOpen,
    ParenClose,
    BraceOpen,
    BraceClose,
    Comma,
    StringLiteral(String),
    Comment(String),
    Whitespace,
}

/// 解析错误
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PsParseError {
    pub message: String,
    pub position: usize,
    pub error_type: PsParseErrorType,
}

/// 解析错误类型
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PsParseErrorType {
    UnexpectedToken,
    UnclosedString,
    UnclosedParenthesis,
    UnclosedBrace,
    InvalidCmdletName,
    IncompleteCommand,
}

/// Cmdlet 规格
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PsCmdletSpec {
    pub name: String,
    pub verb: String,
    pub noun: String,
    pub description: String,
    pub module: Option<String>,
    pub parameters: Vec<PsParameterSpec>,
    pub alias_names: Vec<String>,
    pub common_parameters: bool,
}

/// 参数规格
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PsParameterSpec {
    pub name: String,
    pub short_name: Option<char>,
    pub parameter_type: String,
    pub description: String,
    pub mandatory: bool,
    pub is_switch: bool,
    pub accepts_pipeline_input: bool,
    pub position: Option<i32>,
    pub default_value: Option<String>,
    pub valid_values: Option<Vec<String>>,
}

/// 模块信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PsModuleInfo {
    pub name: String,
    pub version: Option<String>,
    pub path: Option<String>,
    pub exported_cmdlets: Vec<String>,
}

/// PowerShell 解析器
pub struct PowerShellParser {
    cmdlet_regex: Regex,
    param_regex: Regex,
    variable_regex: Regex,
    string_regex: Regex,
    cmdlet_registry: HashMap<String, PsCmdletSpec>,
    module_cache: HashMap<String, PsModuleInfo>,
}

impl Default for PowerShellParser {
    fn default() -> Self {
        let mut parser = Self {
            cmdlet_regex: Regex::new(r"[A-Za-z][A-Za-z0-9]*-[A-Za-z][A-Za-z0-9]*").unwrap(),
            param_regex: Regex::new(r"-[A-Za-z][A-Za-z0-9]*").unwrap(),
            variable_regex: Regex::new(r"\$[A-Za-z_][A-Za-z0-9_]*(?::\w+)?").unwrap(),
            string_regex: Regex::new(r#"['"]"#).unwrap(),
            cmdlet_registry: HashMap::new(),
            module_cache: HashMap::new(),
        };
        parser.register_builtin_cmdlets();
        parser
    }
}

impl PowerShellParser {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn parse(&self, input: &str, cursor_pos: usize) -> PsParseResult {
        let mut tokens = self.tokenize(input);
        let mut errors = vec![];
        let cursor_context = self.analyze_cursor_context(&tokens, cursor_pos, input);

        let ast = if tokens.is_empty() {
            PsAstNode::Empty
        } else {
            match self.build_ast(&mut tokens, &mut errors) {
                Ok(node) => node,
                Err(_) => PsAstNode::Empty,
            }
        };

        PsParseResult { ast, errors, cursor_context }
    }

    pub fn get_suggestions(&self, context: &CompletionContext) -> Vec<CompletionSuggestion> {
        let parse_result = self.parse(&context.line, context.cursor_position);
        let mut suggestions = vec![];

        match &parse_result.cursor_context.current_cmdlet {
            Some(cmd) if cmd.is_empty() || !self.is_valid_cmdlet_prefix(cmd) => {
                let word = context.get_word_before_cursor();
                if word.starts_with('-') {
                    suggestions.extend(self.get_common_parameter_suggestions(word));
                } else if word.starts_with('$') {
                    suggestions.extend(self.get_variable_suggestions(word));
                } else {
                    suggestions.extend(self.get_cmdlet_name_suggestions(word));
                    suggestions.extend(self.get_alias_suggestions(word));
                    if word.is_empty() || word.eq_ignore_ascii_case("import") {
                        suggestions.push(CompletionSuggestion {
                            text: "Import-Module ".to_string(),
                            display_text: "Import-Module".to_string(),
                            description: "导入PowerShell模块".to_string(),
                            kind: CompletionKind::Command,
                            priority: 88,
                            metadata: HashMap::new(),
                        });
                    }
                }
            }
            Some(cmd) => {
                let word = context.get_word_before_cursor();
                if word.starts_with('-') {
                    if let Some(spec) = self.cmdlet_registry.get(cmd) {
                        suggestions.extend(self.get_parameter_suggestions_for_cmdlet(spec, word));
                    }
                    suggestions.extend(self.get_common_parameter_suggestions(word));
                } else if word.starts_with('$') {
                    suggestions.extend(self.get_variable_suggestions(word));
                } else {
                    match cmd.to_lowercase().as_str() {
                        "get-childitem" | "gci" | "dir" | "ls" => {
                            suggestions.extend(self.get_gci_parameter_suggestions(word));
                        }
                        "get-content" | "gc" | "cat" | "type" => {
                            suggestions.extend(self.get_gc_parameter_suggestions(word));
                        }
                        "where-object" | "?" | "where" => {
                            suggestions.extend(self.get_where_object_suggestions(word));
                        }
                        "foreach-object" | "%" | "foreach" => {
                            suggestions.extend(self.get_foreach_object_suggestions(word));
                        }
                        "import-module" => {
                            suggestions.extend(self.get_import_module_suggestions(word));
                        }
                        _ => {
                            if let Some(spec) = self.cmdlet_registry.get(cmd) {
                                for param in &spec.parameters {
                                    if word.is_empty() || format!("-{}", param.name.to_lowercase()).starts_with(&word.to_lowercase()) {
                                        suggestions.push(CompletionSuggestion {
                                            text: if param.is_switch {
                                                format!("{} ", format!("-{}", param.name))
                                            } else {
                                                format!("{} ", format!("-{}", param.name))
                                            },
                                            display_text: format!("-{}", param.name),
                                            description: param.description.clone(),
                                            kind: CompletionKind::Argument,
                                            priority: if param.mandatory { 95 } else { 80 },
                                            metadata: {
                                                let mut m = HashMap::new();
                                                m.insert("type".to_string(), param.parameter_type.clone());
                                                if param.mandatory {
                                                    m.insert("mandatory".to_string(), "true".to_string());
                                                }
                                                m
                                            },
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
            }
            None => {
                suggestions.extend(self.get_cmdlet_name_suggestions(""));
            }
        }

        if parse_result.cursor_context.in_pipeline {
            suggestions.extend(self.get_pipeline_cmdlet_suggestions(context.get_word_before_cursor()));
        }

        suggestions
    }

    fn tokenize(&self, input: &str) -> Vec<PsToken> {
        let mut tokens = vec![];
        let mut pos = 0;
        let chars: Vec<char> = input.chars().collect();

        while pos < chars.len() {
            let ch = chars[pos];

            match ch {
                ' ' | '\t' | '\r' | '\n' => {
                    tokens.push(PsToken::Whitespace);
                    pos += 1;
                }
                '|' => {
                    if pos + 1 < chars.len() && chars[pos + 1] == '|' {
                        tokens.push(PsToken::Parameter("-or".into()));
                        pos += 2;
                    } else {
                        tokens.push(PsToken::Pipe);
                        pos += 1;
                    }
                }
                ';' => {
                    tokens.push(PsToken::Semicolon);
                    pos += 1;
                }
                '=' => {
                    tokens.push(PsToken::Equals);
                    pos += 1;
                }
                '(' => {
                    if pos > 0 && chars[pos - 1] == '$' {
                        if let Some(last) = tokens.last_mut() {
                            if matches!(last, PsToken::Variable(_)) {
                                tokens.pop();
                                tokens.push(PsToken::DollarParenOpen);
                                pos += 1;
                                continue;
                            }
                        }
                    }
                    tokens.push(PsToken::ParenOpen);
                    pos += 1;
                }
                ')' => {
                    tokens.push(PsToken::ParenClose);
                    pos += 1;
                }
                '{' => {
                    tokens.push(PsToken::BraceOpen);
                    pos += 1;
                }
                '}' => {
                    tokens.push(PsToken::BraceClose);
                    pos += 1;
                }
                ',' => {
                    tokens.push(PsToken::Comma);
                    pos += 1;
                }
                '$' => {
                    let var_start = pos;
                    pos += 1;

                    if pos < chars.len() && chars[pos] == '(' {
                        tokens.push(PsToken::DollarParenOpen);
                        pos += 1;
                        continue;
                    }

                    if pos < chars.len() && chars[pos] == '{' {
                        pos += 1;
                        while pos < chars.len() && chars[pos] != '}' {
                            pos += 1;
                        }
                        if pos < chars.len() {
                            pos += 1;
                        }
                        let var_text: String = chars[var_start..pos].iter().collect();
                        tokens.push(PsToken::Variable(var_text));
                        continue;
                    }

                    let mut var_name = String::from("$");
                    while pos < chars.len() && (chars[pos].is_alphanumeric() || chars[pos] == '_' || chars[pos] == ':') {
                        var_name.push(chars[pos]);
                        pos += 1;
                    }

                    if var_name.len() > 1 {
                        tokens.push(PsToken::Variable(var_name));
                    } else {
                        tokens.push(PsToken::Variable("$".into()));
                    }
                }
                '\'' | '"' => {
                    let quote_ch = ch;
                    pos += 1;
                    let mut content = String::new();
                    while pos < chars.len() {
                        if chars[pos] == quote_ch {
                            if pos + 1 < chars.len() && chars[pos + 1] == quote_ch {
                                content.push(quote_ch);
                                pos += 2;
                            } else {
                                break;
                            }
                        } else {
                            content.push(chars[pos]);
                            pos += 1;
                        }
                    }
                    if pos < chars.len() {
                        pos += 1;
                    }
                    tokens.push(PsToken::StringLiteral(content));
                }
                '#' => {
                    let comment_start = pos;
                    while pos < chars.len() && chars[pos] != '\n' {
                        pos += 1;
                    }
                    let comment: String = chars[comment_start..pos].iter().collect();
                    tokens.push(PsToken::Comment(comment));
                }
                '-' => {
                    let param_start = pos;
                    pos += 1;
                    while pos < chars.len() && (chars[pos].is_alphanumeric() || chars[pos] == '_') {
                        pos += 1;
                    }
                    let param_name: String = chars[param_start..pos].iter().collect();
                    tokens.push(PsToken::Parameter(param_name));
                }
                _ if ch.is_alphanumeric() || ch == '.' || ch == '\\' || ch == '/' => {
                    let word_start = pos;
                    while pos < chars.len() &&
                          !chars[pos].is_whitespace() &&
                          !"|;=(){},\"'$#-".contains(chars[pos])
                    {
                        pos += 1;
                    }
                    let word: String = chars[word_start..pos].iter().collect();
                    if self.cmdlet_regex.is_match(&word) {
                        tokens.push(PsToken::Cmdlet(word));
                    } else {
                        tokens.push(PsToken::ParameterValue(word));
                    }
                }
                _ => {
                    pos += 1;
                }
            }
        }

        tokens
    }

    fn analyze_cursor_context(
        &self,
        tokens: &[PsToken],
        cursor_pos: usize,
        input: &str,
    ) -> PsCursorContext {
        let mut current_cmdlet = None;
        let mut current_param_index = None;
        let mut in_pipeline = false;
        let mut pipeline_position = None;
        let mut pipeline_count = 0;
        let mut tokens_before_cursor = vec![];
        let mut char_count = 0usize;

        for token in tokens {
            let token_len = self.token_len(token);

            if char_count + token_len <= cursor_pos {
                tokens_before_cursor.push(token.clone());
                char_count += token_len;

                match token {
                    PsToken::Cmdlet(name) | PsToken::ParameterValue(name) if current_cmdlet.is_none() => {
                        if self.cmdlet_registry.contains_key(name) || self.is_cmdlet_like(name) {
                            current_cmdlet = Some(name.clone());
                            current_param_index = Some(0);
                        } else {
                            current_cmdlet = Some(name.clone());
                            current_param_index = Some(0);
                        }
                    }
                    PsToken::Parameter(_) | PsToken::ParameterValue(_) if current_cmdlet.is_some() => {
                        if let Some(ref mut idx) = current_param_index {
                            *idx += 1;
                        }
                    }
                    PsToken::Pipe => {
                        in_pipeline = true;
                        pipeline_count += 1;
                        pipeline_position = Some(pipeline_count);
                        current_cmdlet = None;
                        current_param_index = None;
                    }
                    PsToken::Semicolon => {
                        in_pipeline = false;
                        current_cmdlet = None;
                        current_param_index = None;
                    }
                    _ => {}
                }
            } else {
                break;
            }
        }

        let before_cursor: String = input[..cursor_pos.min(input.len())].chars().collect();
        let mut in_variable_expansion = false;
        let mut in_string = false;
        let mut string_type = None;

        for ch in before_cursor.chars().rev() {
            if ch == '"' || ch == '\'' {
                in_string = true;
                string_type = Some(ch);
                break;
            }
            if ch == '$' {
                in_variable_expansion = true;
                break;
            }
        }

        PsCursorContext {
            current_cmdlet,
            current_param_index,
            in_pipeline,
            pipeline_position,
            tokens_before_cursor,
            in_variable_expansion,
            in_string,
            string_type,
        }
    }

    fn build_ast(&self, tokens: &mut Vec<PsToken>, _errors: &mut Vec<PsParseError>) -> Result<PsAstNode, ()> {
        let mut statements = vec![];
        let mut current_cmd_words = vec![];

        while !tokens.is_empty() {
            match tokens.remove(0) {
                PsToken::Pipe => {
                    if !current_cmd_words.is_empty() {
                        statements.push(self.build_cmdlet_node(current_cmd_words));
                        current_cmd_words = vec![];
                    }
                }
                PsToken::Semicolon => {
                    if !current_cmd_words.is_empty() {
                        statements.push(self.build_cmdlet_node(current_cmd_words));
                        current_cmd_words = vec![];
                    }
                }
                PsToken::Cmdlet(w) | PsToken::Parameter(w) | PsToken::ParameterValue(w) => {
                    current_cmd_words.push(w);
                }
                _ => {}
            }
        }

        if !current_cmd_words.is_empty() {
            statements.push(self.build_cmdlet_node(current_cmd_words));
        }

        if statements.len() == 1 {
            Ok(statements.remove(0))
        } else if !statements.is_empty() {
            Ok(PsAstNode::StatementList(statements))
        } else {
            Ok(PsAstNode::Empty)
        }
    }

    fn build_cmdlet_node(&self, words: Vec<String>) -> PsAstNode {
        if words.is_empty() {
            return PsAstNode::Empty;
        }

        let name = words[0].clone();
        let mut parameters = vec![];
        let mut i = 1;

        while i < words.len() {
            let word = &words[i];
            if word.starts_with('-') {
                let param_name = word[1..].to_string();
                let is_switch = i + 1 >= words.len() || words[i + 1].starts_with('-');
                let value = if !is_switch && i + 1 < words.len() {
                    i += 1;
                    Some(words[i].clone())
                } else {
                    None
                };
                parameters.push(PsParameter {
                    name: Some(param_name),
                    value,
                    is_switch,
                    is_positional: false,
                });
            } else {
                parameters.push(PsParameter {
                    name: None,
                    value: Some(word.clone()),
                    is_switch: false,
                    is_positional: true,
                });
            }
            i += 1;
        }

        PsAstNode::Cmdlet(PsCmdletNode {
            name,
            parameters,
            trailing_space: false,
            range: PsTextRange::default(),
        })
    }

    fn token_len(&self, token: &PsToken) -> usize {
        match token {
            PsToken::Cmdlet(s) | PsToken::Parameter(s) | PsToken::ParameterValue(s)
            | PsToken::Variable(s) | PsToken::StringLiteral(s) | PsToken::Comment(s) => s.len(),
            PsToken::Pipe | PsToken::Semicolon | PsToken::Equals | PsToken::Comma
            | PsToken::DollarParenOpen | PsToken::DollarParenClose | PsToken::ParenOpen
            | PsToken::ParenClose | PsToken::BraceOpen | PsToken::BraceClose
            | PsToken::Whitespace => 1,
        }
    }

    fn is_valid_cmdlet_prefix(&self, s: &str) -> bool {
        s.contains('-') || self.cmdlet_registry.contains_key(s)
    }

    fn is_cmdlet_like(&self, name: &str) -> bool {
        self.cmdlet_regex.is_match(name)
            || name.eq_ignore_ascii_case("dir")
            || name.eq_ignore_ascii_case("ls")
            || name.eq_ignore_ascii_case("echo")
            || name.eq_ignore_ascii_case("cd")
            || name.eq_ignore_ascii_case("cat")
            || name.eq_ignore_ascii_error("type")
            || name.eq_ignore_ascii_case("%")
            || name.eq_ignore_ascii_case("?")
    }

    fn register_builtin_cmdlets(&mut self) {
        let cmdlets: Vec<(PsCmdletSpec)> = vec![
            PsCmdletSpec {
                name: "Get-ChildItem".into(), verb: "Get".into(), noun: "ChildItem".into(),
                description: "获取目录/文件列表".into(), module: Some("Microsoft.PowerShell.Management".into()),
                parameters: vec![
                    PsParameterSpec { name: "Path".into(), short_name: None, parameter_type: "string[]".into(),
                        description: "要列出的路径".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: true, position: Some(0), default_value: None, valid_values: None },
                    PsParameterSpec { name: "Filter".into(), short_name: None, parameter_type: "string".into(),
                        description: "过滤条件".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "Recurse".into(), short_name: None, parameter_type: "switch".into(),
                        description: "递归子目录".into(), mandatory: false, is_switch: true,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "Force".into(), short_name: None, parameter_type: "switch".into(),
                        description: "显示隐藏/系统文件".into(), mandatory: false, is_switch: true,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "Name".into(), short_name: None, parameter_type: "switch".into(),
                        description: "只返回名称".into(), mandatory: false, is_switch: true,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "File".into(), short_name: None, parameter_type: "switch".into(),
                        description: "只返回文件".into(), mandatory: false, is_switch: true,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "Directory".into(), short_name: None, parameter_type: "switch".into(),
                        description: "只返回目录".into(), mandatory: false, is_switch: true,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "Hidden".into(), short_name: None, parameter_type: "switch".into(),
                        description: "只返回隐藏项".into(), mandatory: false, is_switch: true,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "ReadOnly".into(), short_name: None, parameter_type: "switch".into(),
                        description: "只返回只读项".into(), mandatory: false, is_switch: true,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "System".into(), short_name: None, parameter_type: "switch".into(),
                        description: "只返回系统文件".into(), mandatory: false, is_switch: true,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "Depth".into(), short_name: None, parameter_type: "int".into(),
                        description: "递归深度".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                ],
                alias_names: vec!["gci".into(), "dir".into(), "ls".into()],
                common_parameters: true,
            },
            PsCmdletSpec {
                name: "Get-Content".into(), verb: "Get".into(), noun: "Content".into(),
                description: "获取文件内容".into(), module: Some("Microsoft.PowerShell.Management".into()),
                parameters: vec![
                    PsParameterSpec { name: "Path".into(), short_name: None, parameter_type: "string[]".into(),
                        description: "文件路径".into(), mandatory: true, is_switch: false,
                        accepts_pipeline_input: true, position: Some(0), default_value: None, valid_values: None },
                    PsParameterSpec { name: "TotalCount".into(), short_name: Some('h'), parameter_type: "int".into(),
                        description: "读取行数(从头)".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "Tail".into(), short_name: Some('t'), parameter_type: "int".into(),
                        description: "读取最后N行".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "Encoding".into(), short_name: None, parameter_type: "string".into(),
                        description: "编码格式".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None,
                        valid_values: Some(vec!["ascii".into(),"utf8".into(),"unicode".into(),"utf32".into(),"utf7".into()]) },
                    PsParameterSpec { name: "Raw".into(), short_name: None, parameter_type: "switch".into(),
                        description: "忽略换行符".into(), mandatory: false, is_switch: true,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "Wait".into(), short_name: Some('w'), parameter_type: "switch".into(),
                        description: "等待新内容".into(), mandatory: false, is_switch: true,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                ],
                alias_names: vec!["gc".into(), "cat".into(), "type".into()],
                common_parameters: true,
            },
            PsCmdletSpec {
                name: "Write-Host".into(), verb: "Write".into(), noun: "Host".into(),
                description: "输出到控制台".into(), module: Some("Microsoft.PowerShell.Utility".into()),
                parameters: vec![
                    PsParameterSpec { name: "Object".into(), short_name: None, parameter_type: "object".into(),
                        description: "输出对象".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: true, position: Some(0), default_value: None, valid_values: None },
                    PsParameterSpec { name: "Separator".into(), short_name: None, parameter_type: "string".into(),
                        description: "分隔符字符串".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "ForegroundColor".into(), short_name: None, parameter_type: "ConsoleColor".into(),
                        description: "前景色".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None,
                        valid_values: Some(vec!["Black".into(),"DarkBlue".into(),"DarkGreen".into(),"DarkCyan".into(),
                                               "DarkRed".into(),"DarkMagenta".into(),"DarkYellow".into(),"Gray".into(),
                                               "DarkGray".into(),"Blue".into(),"Green".into(),"Cyan".into(),
                                               "Red".into(),"Magenta".into(),"Yellow".into(),"White".into()]) },
                    PsParameterSpec { name: "BackgroundColor".into(), short_name: None, parameter_type: "ConsoleColor".into(),
                        description: "背景色".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "NoNewline".into(), short_name: None, parameter_type: "switch".into(),
                        description: "不追加换行符".into(), mandatory: false, is_switch: true,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                ],
                alias_names: vec!["write".into(), "echo".into()],
                common_parameters: true,
            },
            PsCmdletSpec {
                name: "Where-Object".into(), verb: "Where".into(), noun: "Object".into(),
                description: "从集合中选择对象".into(), module: Some("Microsoft.PowerShell.Core".into()),
                parameters: vec![
                    PsParameterSpec { name: "FilterScript".into(), short_name: None, parameter_type: "ScriptBlock".into(),
                        description: "筛选条件脚本块".into(), mandatory: true, is_switch: false,
                        accepts_pipeline_input: false, position: Some(0), default_value: None, valid_values: None },
                    PsParameterSpec { name: "Property".into(), short_name: None, parameter_type: "string".into(),
                        description: "属性名".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "Value".into(), short_name: None, parameter_type: "object".into(),
                        description: "属性值".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "EQ".into(), short_name: None, parameter_type: "switch".into(),
                        description: "等于".into(), mandatory: false, is_switch: true,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "NE".into(), short_name: None, parameter_type: "switch".into(),
                        description: "不等于".into(), mandatory: false, is_switch: true,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "GT".into(), short_name: None, parameter_type: "switch".into(),
                        description: "大于".into(), mandatory: false, is_switch: true,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "LT".into(), short_name: None, parameter_type: "switch".into(),
                        description: "小于".into(), mandatory: false, is_switch: true,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "Like".into(), short_name: None, parameter_type: "switch".into(),
                        description: "通配匹配".into(), mandatory: false, is_switch: true,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "Match".into(), short_name: None, parameter_type: "switch".into(),
                        description: "正则匹配".into(), mandatory: false, is_switch: true,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "Contains".into(), short_name: None, parameter_type: "switch".into(),
                        description: "包含".into(), mandatory: false, is_switch: true,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "Not".into(), short_name: None, parameter_type: "switch".into(),
                        description: "取反".into(), mandatory: false, is_switch: true,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                ],
                alias_names: vec!["?".into(), "where".into()],
                common_parameters: true,
            },
            PsCmdletSpec {
                name: "ForEach-Object".into(), verb: "ForEach".into(), noun: "Object".into(),
                description: "对每个输入对象执行操作".into(), module: Some("Microsoft.PowerShell.Core".into()),
                parameters: vec![
                    PsParameterSpec { name: "Process".into(), short_name: None, parameter_type: "ScriptBlock[]".into(),
                        description: "处理脚本块".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: Some(0), default_value: None, valid_values: None },
                    PsParameterSpec { name: "Begin".into(), short_name: None, parameter_type: "ScriptBlock".into(),
                        description: "开始前执行".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "End".into(), short_name: None, parameter_type: "ScriptBlock".into(),
                        description: "结束后执行".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "MemberName".into(), short_name: None, parameter_type: "string".into(),
                        description: "调用的属性/方法名".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "InputObject".into(), short_name: None, parameter_type: "psobject".into(),
                        description: "输入对象".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: true, position: Some(-1), default_value: None, valid_values: None },
                ],
                alias_names: vec!["%".into(), "foreach".into()],
                common_parameters: true,
            },
            PsCmdletSpec {
                name: "Select-Object".into(), verb: "Select".into(), noun: "Object".into(),
                description: "选择对象的属性或对象".into(), module: Some("Microsoft.PowerShell.Utility".into()),
                parameters: vec![
                    PsParameterSpec { name: "Property".into(), short_name: None, parameter_type: "object[]".into(),
                        description: "选择属性".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "First".into(), short_name: None, parameter_type: "int".into(),
                        description: "前N个对象".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "Last".into(), short_name: None, parameter_type: "int".into(),
                        description: "后N个对象".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "Skip".into(), short_name: None, parameter_type: "int".into(),
                        description: "跳过前N个".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "Unique".into(), short_name: None, parameter_type: "switch".into(),
                        description: "去重".into(), mandatory: false, is_switch: true,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "ExpandProperty".into(), short_name: None, parameter_type: "string".into(),
                        description: "展开属性".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                ],
                alias_names: vec!["select".into()],
                common_parameters: true,
            },
            PsCmdletSpec {
                name: "Sort-Object".into(), verb: "Sort".into(), noun: "Object".into(),
                description: "排序对象".into(), module: Some("Microsoft.PowerShell.Utility".into()),
                parameters: vec![
                    PsParameterSpec { name: "Property".into(), short_name: None, parameter_type: "object[]".into(),
                        description: "排序属性".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: Some(0), default_value: None, valid_values: None },
                    PsParameterSpec { name: "Descending".into(), short_name: None, parameter_type: "switch".into(),
                        description: "降序排列".into(), mandatory: false, is_switch: true,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "Unique".into(), short_name: None, parameter_type: "switch".into(),
                        description: "去重后排序".into(), mandatory: false, is_switch: true,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                ],
                alias_names: vec!["sort".into()],
                common_parameters: true,
            },
            PsCmdletSpec {
                name: "Group-Object".into(), verb: "Group".into(), noun: "Object".into(),
                description: "分组对象".into(), module: Some("Microsoft.PowerShell.Utility".into()),
                parameters: vec![
                    PsParameterSpec { name: "Property".into(), short_name: None, parameter_type: "object[]".into(),
                        description: "分组属性".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "AsHashTable".into(), short_name: None, parameter_type: "switch".into(),
                        description: "返回哈希表".into(), mandatory: false, is_switch: true,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                ],
                alias_names: vec!["group".into()],
                common_parameters: true,
            },
            PsCmdletSpec {
                name: "Measure-Object".into(), verb: "Measure".into(), noun: "Object".into(),
                description: "计算数值属性统计".into(), module: Some("Microsoft.PowerShell.Utility".into()),
                parameters: vec![
                    PsParameterSpec { name: "Property".into(), short_name: None, parameter_type: "string[]".into(),
                        description: "测量属性".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "Sum".into(), short_name: None, parameter_type: "switch".into(),
                        description: "求和".into(), mandatory: false, is_switch: true,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "Average".into(), short_name: None, parameter_type: "switch".into(),
                        description: "平均值".into(), mandatory: false, is_switch: true,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "Maximum".into(), short_name: None, parameter_type: "switch".into(),
                        description: "最大值".into(), mandatory: false, is_switch: true,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "Minimum".into(), short_name: None, parameter_type: "switch".into(),
                        description: "最小值".into(), mandatory: false, is_switch: true,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                ],
                alias_names: vec!["measure".into()],
                common_parameters: true,
            },
            PsCmdletSpec {
                name: "Compare-Object".into(), verb: "Compare".into(), noun: "Object".into(),
                description: "比较两个对象集".into(), module: Some("Microsoft.PowerShell.Utility".into()),
                parameters: vec![
                    PsParameterSpec { name: "ReferenceObject".into(), short_name: None, parameter_type: "psobject".into(),
                        description: "参考对象".into(), mandatory: true, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "DifferenceObject".into(), short_name: None, parameter_type: "psobject".into(),
                        description: "差异对象".into(), mandatory: true, is_switch: false,
                        accepts_pipeline_input: true, position: Some(0), default_value: None, valid_values: None },
                    PsParameterSpec { name: "Property".into(), short_name: None, parameter_type: "object[]".into(),
                        description: "比较属性".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                ],
                alias_names: vec!["compare".into(), "diff".into()],
                common_parameters: true,
            },
            PsCmdletSpec {
                name: "Set-Content".into(), verb: "Set".into(), noun: "Content".into(),
                description: "设置文件内容(覆盖)".into(), module: Some("Microsoft.PowerShell.Management".into()),
                parameters: vec![
                    PsParameterSpec { name: "Path".into(), short_name: None, parameter_type: "string[]".into(),
                        description: "目标路径".into(), mandatory: true, is_switch: false,
                        accepts_pipeline_input: true, position: Some(0), default_value: None, valid_values: None },
                    PsParameterSpec { name: "Value".into(), short_name: None, parameter_type: "object[]".into(),
                        description: "写入内容".into(), mandatory: true, is_switch: false,
                        accepts_pipeline_input: true, position: Some(1), default_value: None, valid_values: None },
                    PsParameterSpec { name: "Encoding".into(), short_name: None, parameter_type: "string".into(),
                        description: "编码".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                ],
                alias_names: vec!["sc".into()],
                common_parameters: true,
            },
            PsCmdletSpec {
                name: "Add-Content".into(), verb: "Add".into(), noun: "Content".into(),
                description: "追加内容到文件".into(), module: Some("Microsoft.PowerShell.Management".into()),
                parameters: vec![
                    PsParameterSpec { name: "Path".into(), short_name: None, parameter_type: "string[]".into(),
                        description: "目标路径".into(), mandatory: true, is_switch: false,
                        accepts_pipeline_input: true, position: Some(0), default_value: None, valid_values: None },
                    PsParameterSpec { name: "Value".into(), short_name: None, parameter_type: "object[]".into(),
                        description: "追加内容".into(), mandatory: true, is_switch: false,
                        accepts_pipeline_input: true, position: Some(1), default_value: None, valid_values: None },
                ],
                alias_names: vec!["ac".into()],
                common_parameters: true,
            },
            PsCmdletSpec {
                name: "New-Item".into(), verb: "New".into(), noun: "Item".into(),
                description: "创建新项目".into(), module: Some("Microsoft.PowerShell.Management".into()),
                parameters: vec![
                    PsParameterSpec { name: "Path".into(), short_name: None, parameter_type: "string[]".into(),
                        description: "创建路径".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: true, position: Some(0), default_value: None, valid_values: None },
                    PsParameterSpec { name: "Name".into(), short_name: None, parameter_type: "string".into(),
                        description: "项目名称".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: true, position: Some(1), default_value: None, valid_values: None },
                    PsParameterSpec { name: "ItemType".into(), short_name: None, parameter_type: "string".into(),
                        description: "项目类型".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None,
                        valid_values: Some(vec!["File".into(),"Directory".into(),"SymbolicLink".into(),"Junction".into()]) },
                    PsParameterSpec { name: "Value".into(), short_name: None, parameter_type: "object".into(),
                        description: "初始值".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "Force".into(), short_name: None, parameter_type: "switch".into(),
                        description: "强制创建".into(), mandatory: false, is_switch: true,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                ],
                alias_names: vec!["ni".into()],
                common_parameters: true,
            },
            PsCmdletSpec {
                name: "Remove-Item".into(), verb: "Remove".into(), noun: "Item".into(),
                description: "删除项目".into(), module: Some("Microsoft.PowerShell.Management".into()),
                parameters: vec![
                    PsParameterSpec { name: "Path".into(), short_name: None, parameter_type: "string[]".into(),
                        description: "删除路径".into(), mandatory: true, is_switch: false,
                        accepts_pipeline_input: true, position: Some(0), default_value: None, valid_values: None },
                    PsParameterSpec { name: "Recurse".into(), short_name: None, parameter_type: "switch".into(),
                        description: "递归删除".into(), mandatory: false, is_switch: true,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "Force".into(), short_name: None, parameter_type: "switch".into(),
                        description: "强制删除只读项".into(), mandatory: false, is_switch: true,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                ],
                alias_names: vec!["ri".into(), "del".into(), "rm".into(), "rmdir".into()],
                common_parameters: true,
            },
            PsCmdletSpec {
                name: "Copy-Item".into(), verb: "Copy".into(), noun: "Item".into(),
                description: "复制项目".into(), module: Some("Microsoft.PowerShell.Management".into()),
                parameters: vec![
                    PsParameterSpec { name: "Path".into(), short_name: None, parameter_type: "string[]".into(),
                        description: "源路径".into(), mandatory: true, is_switch: false,
                        accepts_pipeline_input: true, position: Some(0), default_value: None, valid_values: None },
                    PsParameterSpec { name: "Destination".into(), short_name: None, parameter_type: "string".into(),
                        description: "目标路径".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "Recurse".into(), short_name: None, parameter_type: "switch".into(),
                        description: "递归复制".into(), mandatory: false, is_switch: true,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "Force".into(), short_name: None, parameter_type: "switch".into(),
                        description: "强制覆盖".into(), mandatory: false, is_switch: true,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                ],
                alias_names: vec!["copy".into(), "cp".into(), "cpi".into()],
                common_parameters: true,
            },
            PsCmdletSpec {
                name: "Move-Item".into(), verb: "Move".into(), noun: "Item".into(),
                description: "移动项目".into(), module: Some("Microsoft.PowerShell.Management".into()),
                parameters: vec![
                    PsParameterSpec { name: "Path".into(), short_name: None, parameter_type: "string[]".into(),
                        description: "源路径".into(), mandatory: true, is_switch: false,
                        accepts_pipeline_input: true, position: Some(0), default_value: None, valid_values: None },
                    PsParameterSpec { name: "Destination".into(), short_name: None, parameter_type: "string".into(),
                        description: "目标路径".into(), mandatory: true, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "Force".into(), short_name: None, parameter_type: "switch".into(),
                        description: "强制移动".into(), mandatory: false, is_switch: true,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                ],
                alias_names: vec!["move".into(), "mv".into(), "mi".into()],
                common_parameters: true,
            },
            PsCmdletSpec {
                name: "Rename-Item".into(), verb: "Rename".into(), noun: "Item".into(),
                description: "重命名项目".into(), module: Some("Microsoft.PowerShell.Management".into()),
                parameters: vec![
                    PsParameterSpec { name: "Path".into(), short_name: None, parameter_type: "string".into(),
                        description: "源路径".into(), mandatory: true, is_switch: false,
                        accepts_pipeline_input: true, position: Some(0), default_value: None, valid_values: None },
                    PsParameterSpec { name: "NewName".into(), short_name: None, parameter_type: "string".into(),
                        description: "新名称".into(), mandatory: true, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "Force".into(), short_name: None, parameter_type: "switch".into(),
                        description: "强制重命名".into(), mandatory: false, is_switch: true,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                ],
                alias_names: vec!["ren".into(), "rni".into()],
                common_parameters: true,
            },
            PsCmdletSpec {
                name: "Get-Process".into(), verb: "Get".into(), noun: "Process".into(),
                description: "获取进程列表".into(), module: Some("Microsoft.PowerShell.Management".into()),
                parameters: vec![
                    PsParameterSpec { name: "Name".into(), short_name: None, parameter_type: "string[]".into(),
                        description: "进程名".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: true, position: Some(0), default_value: None, valid_values: None },
                    PsParameterSpec { name: "Id".into(), short_name: None, parameter_type: "int[]".into(),
                        description: "进程ID".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: true, position: None, default_value: None, valid_values: None },
                ],
                alias_names: vec!["ps".into(), "gps".into()],
                common_parameters: true,
            },
            PsCmdletSpec {
                name: "Stop-Process".into(), verb: "Stop".into(), noun: "Process".into(),
                description: "终止进程".into(), module: Some("Microsoft.PowerShell.Management".into()),
                parameters: vec![
                    PsParameterSpec { name: "Name".into(), short_name: None, parameter_type: "string[]".into(),
                        description: "进程名".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: true, position: Some(0), default_value: None, valid_values: None },
                    PsParameterSpec { name: "Id".into(), short_name: None, parameter_type: "int[]".into(),
                        description: "进程ID".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: true, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "Force".into(), short_name: None, parameter_type: "switch".into(),
                        description: "强制终止".into(), mandatory: false, is_switch: true,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                ],
                alias_names: vec!["kill".into(), "spps".into()],
                common_parameters: true,
            },
            PsCmdletSpec {
                name: "Get-Service".into(), verb: "Get".into(), noun: "Service".into(),
                description: "获取Windows服务".into(), module: Some("Microsoft.PowerShell.Management".into()),
                parameters: vec![
                    PsParameterSpec { name: "Name".into(), short_name: None, parameter_type: "string[]".into(),
                        description: "服务名".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: true, position: Some(0), default_value: None, valid_values: None },
                    PsParameterSpec { name: "DisplayName".into(), short_name: None, parameter_type: "string[]".into(),
                        description: "显示名".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                ],
                alias_names: vec!["gsv".into()],
                common_parameters: true,
            },
            PsCmdletSpec {
                name: "Start-Service".into(), verb: "Start".into(), noun: "Service".into(),
                description: "启动服务".into(), module: Some("Microsoft.PowerShell.Management".into()),
                parameters: vec![
                    PsParameterSpec { name: "Name".into(), short_name: None, parameter_type: "string[]".into(),
                        description: "服务名".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: true, position: Some(0), default_value: None, valid_values: None },
                    PsParameterSpec { name: "Force".into(), short_name: None, parameter_type: "switch".into(),
                        description: "强制启动".into(), mandatory: false, is_switch: true,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                ],
                alias_names: vec!["sasv".into()],
                common_parameters: true,
            },
            PsCmdletSpec {
                name: "Stop-Service".into(), verb: "Stop".into(), noun: "Service".into(),
                description: "停止服务".into(), module: Some("Microsoft.PowerShell.Management".into()),
                parameters: vec![
                    PsParameterSpec { name: "Name".into(), short_name: None, parameter_type: "string[]".into(),
                        description: "服务名".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: true, position: Some(0), default_value: None, valid_values: None },
                    PsParameterSpec { name: "Force".into(), short_name: None, parameter_type: "switch".into(),
                        description: "强制停止".into(), mandatory: false, is_switch: true,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                ],
                alias_names: vec!["spsv".into()],
                common_parameters: true,
            },
            PsCmdletSpec {
                name: "Get-Help".into(), verb: "Get".into(), noun: "Help".into(),
                description: "显示帮助信息".into(), module: Some("Microsoft.PowerShell.Core".into()),
                parameters: vec![
                    PsParameterSpec { name: "Name".into(), short_name: None, parameter_type: "string".into(),
                        description: "命令/主题名".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: true, position: Some(0), default_value: None, valid_values: None },
                    PsParameterSpec { name: "Detailed".into(), short_name: None, parameter_type: "switch".into(),
                        description: "详细信息".into(), mandatory: false, is_switch: true,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "Full".into(), short_name: None, parameter_type: "switch".into(),
                        description: "完整信息".into(), mandatory: false, is_switch: true,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "Examples".into(), short_name: None, parameter_type: "switch".into(),
                        description: "示例".into(), mandatory: false, is_switch: true,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "Online".into(), short_name: None, parameter_type: "switch".into(),
                        description: "打开在线文档".into(), mandatory: false, is_switch: true,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "ShowWindow".into(), short_name: None, parameter_type: "switch".into(),
                        description: "在窗口中显示".into(), mandatory: false, is_switch: true,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                ],
                alias_names: vec!["help".into(), "man".into()],
                common_parameters: true,
            },
            PsCmdletSpec {
                name: "Import-Module".into(), verb: "Import".into(), noun: "Module".into(),
                description: "导入PowerShell模块".into(), module: Some("Microsoft.PowerShell.Core".into()),
                parameters: vec![
                    PsParameterSpec { name: "Name".into(), short_name: None, parameter_type: "string[]".into(),
                        description: "模块名".into(), mandatory: true, is_switch: false,
                        accepts_pipeline_input: true, position: Some(0), default_value: None, valid_values: None },
                    PsParameterSpec { name: "FullyQualifiedName".into(), short_name: None, parameter_type: "ModuleSpecification".into(),
                        description: "完全限定名".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "RequiredVersion".into(), short_name: None, parameter_type: "version".into(),
                        description: "所需版本".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "MinimumVersion".into(), short_name: None, parameter_type: "version".into(),
                        description: "最低版本".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "Force".into(), short_name: None, parameter_type: "switch".into(),
                        description: "强制重新加载".into(), mandatory: false, is_switch: true,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "PassThru".into(), short_name: None, parameter_type: "switch".into(),
                        description: "返回模块对象".into(), mandatory: false, is_switch: true,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "ListAvailable".into(), short_name: None, parameter_type: "switch".into(),
                        description: "列出可用模块".into(), mandatory: false, is_switch: true,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                ],
                alias_names: vec![],
                common_parameters: true,
            },
            PsCmdletSpec {
                name: "Export-ModuleMember".into(), verb: "Export".into(), noun: "ModuleMember".into(),
                description: "导出模块成员".into(), module: Some("Microsoft.PowerShell.Core".into()),
                parameters: vec![
                    PsParameterSpec { name: "Function".into(), short_name: None, parameter_type: "string[]".into(),
                        description: "导出的函数".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "Cmdlet".into(), short_name: None, parameter_type: "string[]".into(),
                        description: "导出的cmdlet".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "Variable".into(), short_name: None, parameter_type: "string[]".into(),
                        description: "导出的变量".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                ],
                alias_names: vec![],
                common_parameters: true,
            },
            PsCmdletSpec {
                name: "Invoke-Command".into(), verb: "Invoke".into(), noun: "Command".into(),
                description: "在本地/远程运行命令".into(), module: Some("Microsoft.PowerShell.Core".into()),
                parameters: vec![
                    PsParameterSpec { name: "ScriptBlock".into(), short_name: None, parameter_type: "ScriptBlock".into(),
                        description: "执行的脚本块".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "ComputerName".into(), short_name: Some('c'), parameter_type: "string[]".into(),
                        description: "远程计算机".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "Session".into(), short_name: None, parameter_type: "PSSession[]".into(),
                        description: "PSSession".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "FilePath".into(), short_name: None, parameter_type: "string".into(),
                        description: "本地脚本路径".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                ],
                alias_names: vec!["icm".into()],
                common_parameters: true,
            },
            PsCmdletSpec {
                name: "Invoke-Expression".into(), verb: "Invoke".into(), noun: "Expression".into(),
                description: "运行字符串作为命令".into(), module: Some("Microsoft.PowerShell.Utility".into()),
                parameters: vec![
                    PsParameterSpec { name: "Command".into(), short_name: None, parameter_type: "string".into(),
                        description: "要运行的命令字符串".into(), mandatory: true, is_switch: false,
                        accepts_pipeline_input: true, position: Some(0), default_value: None, valid_values: None },
                ],
                alias_names: vec!["iex".into()],
                common_parameters: true,
            },
            PsCmdletSpec {
                name: "Start-Sleep".into(), verb: "Start".into(), noun: "Sleep".into(),
                description: "暂停指定时间".into(), module: Some("Microsoft.PowerShell.Utility".into()),
                parameters: vec![
                    PsParameterSpec { name: "Seconds".into(), short_name: Some('s'), parameter_type: "int".into(),
                        description: "秒数".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "Milliseconds".into(), short_name: Some('m'), parameter_type: "int".into(),
                        description: "毫秒数".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                ],
                alias_names: vec!["sleep".into()],
                common_parameters: true,
            },
            PsCmdletSpec {
                name: "Get-Date".into(), verb: "Get".into(), noun: "Date".into(),
                description: "获取当前日期时间".into(), module: Some("Microsoft.PowerShell.Utility".into()),
                parameters: vec![
                    PsParameterSpec { name: "Format".into(), short_name: Some('f'), parameter_type: "string".into(),
                        description: ".NET格式字符串".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "Year".into(), short_name: None, parameter_type: "int".into(),
                        description: "年份".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "Month".into(), short_name: None, parameter_type: "int".into(),
                        description: "月份".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "Day".into(), short_name: None, parameter_type: "int".into(),
                        description: "日期".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "Hour".into(), short_name: None, parameter_type: "int".into(),
                        description: "小时".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "UFormat".into(), short_name: Some('u'), parameter_type: "string".into(),
                        description: "Unix格式字符串".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                ],
                alias_names: vec![],
                common_parameters: true,
            },
            PsCmdletSpec {
                name: "Set-Location".into(), verb: "Set".into(), noun: "Location".into(),
                description: "切换工作目录".into(), module: Some("Microsoft.PowerShell.Management".into()),
                parameters: vec![
                    PsParameterSpec { name: "Path".into(), short_name: None, parameter_type: "string".into(),
                        description: "目标路径".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: true, position: Some(0), default_value: None, valid_values: None },
                    PsParameterSpec { name: "PassThru".into(), short_name: None, parameter_type: "switch".into(),
                        description: "传递路径对象".into(), mandatory: false, is_switch: true,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                ],
                alias_names: vec!["cd".into(), "chdir".into(), "sl".into()],
                common_parameters: true,
            },
            PsCmdletSpec {
                name: "Get-Location".into(), verb: "Get".into(), noun: "Location".into(),
                description: "获取当前目录".into(), module: Some("Microsoft.PowerShell.Management".into()),
                parameters: vec![
                    PsParameterSpec { name: "PSDrive".into(), short_name: None, parameter_type: "string[]".into(),
                        description: "PS驱动器".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "PSProvider".into(), short_name: None, parameter_type: "string[]".into(),
                        description: "PS提供程序".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "Stack".into(), short_name: None, parameter_type: "switch".into(),
                        description: "显示目录栈".into(), mandatory: false, is_switch: true,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                ],
                alias_names: vec!["gl".into(), "pwd".into()],
                common_parameters: true,
            },
            PsCmdletSpec {
                name: "Push-Location".into(), verb: "Push".into(), noun: "Location".into(),
                description: "压入当前目录到栈".into(), module: Some("Microsoft.PowerShell.Management".into()),
                parameters: vec![
                    PsParameterSpec { name: "Path".into(), short_name: None, parameter_type: "string".into(),
                        description: "目标路径".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                ],
                alias_names: vec!["pushd".into()],
                common_parameters: true,
            },
            PsCmdletSpec {
                name: "Pop-Location".into(), verb: "Pop".into(), noun: "Location".into(),
                description: "弹出栈顶目录".into(), module: Some("Microsoft.PowerShell.Management".into()),
                parameters: vec![
                    PsParameterSpec { name: "StackName".into(), short_name: None, parameter_type: "string".into(),
                        description: "栈名".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                ],
                alias_names: vec!["popd".into()],
                common_parameters: true,
            },
            PsCmdletSpec {
                name: "Test-Path".into(), verb: "Test".into(), noun: "Path".into(),
                description: "检查路径是否存在".into(), module: Some("Microsoft.PowerShell.Management".into()),
                parameters: vec![
                    PsParameterSpec { name: "Path".into(), short_name: None, parameter_type: "string[]".into(),
                        description: "检查路径".into(), mandatory: true, is_switch: false,
                        accepts_pipeline_input: true, position: Some(0), default_value: None, valid_values: None },
                    PsParameterSpec { name: "PathType".into(), short_name: None, parameter_type: "TestPathType".into(),
                        description: "路径类型".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None,
                        valid_values: Some(vec!["Container".into(),"Leaf".into(),"Any".into()]) },
                ],
                alias_names: vec!["testpath".into()],
                common_parameters: true,
            },
            PsCmdletSpec {
                name: "Join-Path".into(), verb: "Join".into(), noun: "Path".into(),
                description: "合并路径".into(), module: Some("Microsoft.PowerShell.Management".into()),
                parameters: vec![
                    PsParameterSpec { name: "Path".into(), short_name: None, parameter_type: "string".into(),
                        description: "父路径".into(), mandatory: true, is_switch: false,
                        accepts_pipeline_input: true, position: Some(0), default_value: None, valid_values: None },
                    PsParameterSpec { name: "ChildPath".into(), short_name: None, parameter_type: "string".into(),
                        description: "子路径".into(), mandatory: true, is_switch: false,
                        accepts_pipeline_input: true, position: Some(1), default_value: None, valid_values: None },
                ],
                alias_names: vec!["join".into()],
                common_parameters: true,
            },
            PsCmdletSpec {
                name: "Split-Path".into(), verb: "Split".into(), noun: "Path".into(),
                description: "拆分路径".into(), module: Some("Microsoft.PowerShell.Management".into()),
                parameters: vec![
                    PsParameterSpec { name: "Path".into(), short_name: None, parameter_type: "string[]".into(),
                        description: "路径".into(), mandatory: true, is_switch: false,
                        accepts_pipeline_input: true, position: Some(0), default_value: None, valid_values: None },
                    PsParameterSpec { name: "Qualifier".into(), short_name: None, parameter_type: "switch".into(),
                        description: "返回驱动器号".into(), mandatory: false, is_switch: true,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "Leaf".into(), short_name: None, parameter_type: "switch".into(),
                        description: "返回末尾元素".into(), mandatory: false, is_switch: true,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "Parent".into(), short_name: None, parameter_type: "switch".into(),
                        description: "返回父路径".into(), mandatory: false, is_switch: true,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "Extension".into(), short_name: None, parameter_type: "switch".into(),
                        description: "返回扩展名".into(), mandatory: false, is_switch: true,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                ],
                alias_names: vec!["split".into()],
                common_parameters: true,
            },
            PsCmdletSpec {
                name: "ConvertFrom-Json".into(), verb: "ConvertFrom".into(), noun: "Json".into(),
                description: "JSON转对象".into(), module: Some("Microsoft.PowerShell.Utility".into()),
                parameters: vec![
                    PsParameterSpec { name: "InputObject".into(), short_name: None, parameter_type: "string".into(),
                        description: "JSON字符串".into(), mandatory: true, is_switch: false,
                        accepts_pipeline_input: true, position: Some(0), default_value: None, valid_values: None },
                ],
                alias_names: vec![],
                common_parameters: true,
            },
            PsCmdletSpec {
                name: "ConvertTo-Json".into(), verb: "ConvertTo".into(), noun: "Json".into(),
                description: "对象转JSON".into(), module: Some("Microsoft.PowerShell.Utility".into()),
                parameters: vec![
                    PsParameterSpec { name: "InputObject".into(), short_name: None, parameter_type: "object".into(),
                        description: "转换对象".into(), mandatory: true, is_switch: false,
                        accepts_pipeline_input: true, position: Some(0), default_value: None, valid_values: None },
                    PsParameterSpec { name: "Depth".into(), short_name: None, parameter_type: "int".into(),
                        description: "序列化深度".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "Compress".into(), short_name: None, parameter_type: "switch".into(),
                        description: "压缩输出".into(), mandatory: false, is_switch: true,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                ],
                alias_names: vec![],
                common_parameters: true,
            },
            PsCmdletSpec {
                name: "ConvertFrom-Csv".into(), verb: "ConvertFrom".into(), noun: "Csv".into(),
                description: "CSV转对象".into(), module: Some("Microsoft.PowerShell.Utility".into()),
                parameters: vec![
                    PsParameterSpec { name: "Path".into(), short_name: None, parameter_type: "string[]".into(),
                        description: "CSV文件路径".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "Delimiter".into(), short_name: None, parameter_type: "char".into(),
                        description: "分隔符".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: Some(",".into()), valid_values: None },
                    PsParameterSpec { name: "Header".into(), short_name: None, parameter_type: "string[]".into(),
                        description: "自定义列头".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                ],
                alias_names: vec![],
                common_parameters: true,
            },
            PsCmdletSpec {
                name: "ConvertTo-Csv".into(), verb: "ConvertTo".into(), noun: "Csv".into(),
                description: "对象转CSV".into(), module: Some("Microsoft.PowerShell.Utility".into()),
                parameters: vec![
                    PsParameterSpec { name: "InputObject".into(), short_name: None, parameter_type: "psobject".into(),
                        description: "转换对象".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: true, position: Some(0), default_value: None, valid_values: None },
                    PsParameterSpec { name: "Delimiter".into(), short_name: None, parameter_type: "char".into(),
                        description: "分隔符".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: Some(",".into()), valid_values: None },
                    PsParameterSpec { name: "NoTypeInformation".into(), short_name: None, parameter_type: "switch".into(),
                        description: "省略类型头".into(), mandatory: false, is_switch: true,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                ],
                alias_names: vec!["epcsv".into()],
                common_parameters: true,
            },
            PsCmdletSpec {
                name: "Out-File".into(), verb: "Out".into(), noun: "File".into(),
                description: "输出到文件".into(), module: Some("Microsoft.PowerShell.Utility".into()),
                parameters: vec![
                    PsParameterSpec { name: "FilePath".into(), short_name: None, parameter_type: "string".into(),
                        description: "文件路径".into(), mandatory: true, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "Encoding".into(), short_name: None, parameter_type: "string".into(),
                        description: "编码".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "Append".into(), short_name: None, parameter_type: "switch".into(),
                        description: "追加模式".into(), mandatory: false, is_switch: true,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "Force".into(), short_name: None, parameter_type: "switch".into(),
                        description: "覆盖只读文件".into(), mandatory: false, is_switch: true,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "NoNewline".into(), short_name: None, parameter_type: "switch".into(),
                        description: "不添加换行".into(), mandatory: false, is_switch: true,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                ],
                alias_names: vec![],
                common_parameters: true,
            },
            PsCmdletSpec {
                name: "Out-String".into(), verb: "Out".into(), noun: "String".into(),
                description: "输出为字符串".into(), module: Some("Microsoft.PowerShell.Utility".into()),
                parameters: vec![
                    PsParameterSpec { name: "Width".into(), short_name: None, parameter_type: "int".into(),
                        description: "每行宽度".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "Stream".into(), short_name: None, parameter_type: "switch".into(),
                        description: "逐个输出字符串".into(), mandatory: false, is_switch: true,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                ],
                alias_names: vec![],
                common_parameters: true,
            },
            PsCmdletSpec {
                name: "Out-Null".into(), verb: "Out".into(), noun: "Null".into(),
                description: "丢弃输出".into(), module: Some("Microsoft.PowerShell.Utility".into()),
                parameters: vec![],
                alias_names: vec!["out-null".into()],
                common_parameters: true,
            },
            PsCmdletSpec {
                name: "Out-Default".into(), verb: "Out".into(), noun: "Default".into(),
                description: "默认格式化输出".into(), module: Some("Microsoft.PowerShell.Core".into()),
                parameters: vec![],
                alias_names: vec![],
                common_parameters: true,
            },
            PsCmdletSpec {
                name: "Tee-Object".into(), verb: "Tee".into(), noun: "Object".into(),
                description: "保存到文件并继续传递".into(), module: Some("Microsoft.PowerShell.Utility".into()),
                parameters: vec![
                    PsParameterSpec { name: "FilePath".into(), short_name: None, parameter_type: "string".into(),
                        description: "文件路径".into(), mandatory: true, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "Append".into(), short_name: None, parameter_type: "switch".into(),
                        description: "追加模式".into(), mandatory: false, is_switch: true,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                ],
                alias_names: vec!["tee".into()],
                common_parameters: true,
            },
            PsCmdletSpec {
                name: "Format-Table".into(), verb: "Format".into(), noun: "Table".into(),
                description: "表格格式输出".into(), module: Some("Microsoft.PowerShell.Utility".into()),
                parameters: vec![
                    PsParameterSpec { name: "Property".into(), short_name: None, parameter_type: "object[]".into(),
                        description: "显示属性".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "AutoSize".into(), short_name: None, parameter_type: "switch".into(),
                        description: "自动调整宽度".into(), mandatory: false, is_switch: true,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "Wrap".into(), short_name: None, parameter_type: "switch".into(),
                        description: "自动换行".into(), mandatory: false, is_switch: true,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                ],
                alias_names: vec!["ft".into()],
                common_parameters: true,
            },
            PsCmdletSpec {
                name: "Format-List".into(), verb: "Format".into(), noun: "List".into(),
                description: "列表格式输出".into(), module: Some("Microsoft.PowerShell.Utility".into()),
                parameters: vec![
                    PsParameterSpec { name: "Property".into(), short_name: None, parameter_type: "object[]".into(),
                        description: "显示属性".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                ],
                alias_names: vec!["fl".into()],
                common_parameters: true,
            },
            PsCmdletSpec {
                name: "Format-Wide".into(), verb: "Format".into(), noun: "Wide".into(),
                description: "宽屏格式输出".into(), module: Some("Microsoft.PowerShell.Utility".into()),
                parameters: vec![
                    PsParameterSpec { name: "Property".into(), short_name: None, parameter_type: "string".into(),
                        description: "显示属性".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "Column".into(), short_name: None, parameter_type: "int".into(),
                        description: "列数".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                ],
                alias_names: vec!["fw".into()],
                common_parameters: true,
            },
            PsCmdletSpec {
                name: "Get-Member".into(), verb: "Get".into(), noun: "Member".into(),
                description: "获取对象成员(属性/方法)".into(), module: Some("Microsoft.PowerShell.Utility".into()),
                parameters: vec![
                    PsParameterSpec { name: "InputObject".into(), short_name: None, parameter_type: "psobject".into(),
                        description: "检查的对象".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: true, position: Some(0), default_value: None, valid_values: None },
                    PsParameterSpec { name: "MemberType".into(), short_name: None, parameter_type: "PSMemberTypes".into(),
                        description: "成员类型".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None,
                        valid_values: Some(vec!["AliasProperty".into(),"CodeProperty".into(),"Property".into(),
                                               "NoteProperty".into(),"ScriptProperty".into(),"Properties".into(),
                                               "PropertySet".into(),"Method".into(),"CodeMethod".into(),
                                               "ScriptMethod".into(),"Methods".into(),"ParameterizedProperty".into(),
                                               "MemberSet".into(),"Event".into(),"Dynamic".into(),"All".into()]) },
                    PsParameterSpec { name: "Name".into(), short_name: None, parameter_type: "string[]".into(),
                        description: "成员名过滤器".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                ],
                alias_names: vec!["gm".into()],
                common_parameters: true,
            },
            PsCmdletSpec {
                name: "Get-Variable".into(), verb: "Get".into(), noun: "Variable".into(),
                description: "获取变量".into(), module: Some("Microsoft.PowerShell.Utility".into()),
                parameters: vec![
                    PsParameterSpec { name: "Name".into(), short_name: None, parameter_type: "string[]".into(),
                        description: "变量名".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: true, position: Some(0), default_value: None, valid_values: None },
                    PsParameterSpec { name: "ValueOnly".into(), short_name: None, parameter_type: "switch".into(),
                        description: "只返回值".into(), mandatory: false, is_switch: true,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                ],
                alias_names: vec!["gv".into()],
                common_parameters: true,
            },
            PsCmdletSpec {
                name: "Set-Variable".into(), verb: "Set".into(), noun: "Variable".into(),
                description: "设置变量".into(), module: Some("Microsoft.PowerShell.Utility".into()),
                parameters: vec![
                    PsParameterSpec { name: "Name".into(), short_name: None, parameter_type: "string".into(),
                        description: "变量名".into(), mandatory: true, is_switch: false,
                        accepts_pipeline_input: false, position: Some(0), default_value: None, valid_values: None },
                    PsParameterSpec { name: "Value".into(), short_name: None, parameter_type: "object".into(),
                        description: "变量值".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: true, position: Some(1), default_value: None, valid_values: None },
                    PsParameterSpec { name: "Scope".into(), short_name: None, parameter_type: "string".into(),
                        description: "作用域".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None,
                        valid_values: Some(vec!["Global".into(),"Local".into(),"Script".into(),"Private".into()]) },
                ],
                alias_names: vec!["set".into(), "sv".into()],
                common_parameters: true,
            },
            PsCmdletSpec {
                name: "New-Object".into(), verb: "New".into(), noun: "Object".into(),
                description: "创建.NET/COM对象实例".into(), module: Some("Microsoft.PowerShell.Utility".into()),
                parameters: vec![
                    PsParameterSpec { name: "TypeName".into(), short_name: None, parameter_type: "string".into(),
                        description: "完整类型名".into(), mandatory: true, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "Property".into(), short_name: None, parameter_type: "IDictionary".into(),
                        description: "属性值字典".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "ArgumentList".into(), short_name: None, parameter_type: "object[]".into(),
                        description: "构造函数参数".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                ],
                alias_names: vec!["new".into()],
                common_parameters: true,
            },
            PsCmdletSpec {
                name: "Add-Type".into(), verb: "Add".into(), noun: "Type".into(),
                description: "定义.NET类型".into(), module: Some("Microsoft.PowerShell.Utility".into()),
                parameters: vec![
                    PsParameterSpec { name: "TypeDefinition".into(), short_name: None, parameter_type: "string".into(),
                        description: "C#类型定义".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: true, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "Path".into(), short_name: None, parameter_type: "string[]".into(),
                        description: "源文件路径".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "AssemblyName".into(), short_name: None, parameter_type: "string[]".into(),
                        description: "程序集名".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                    PsParameterSpec { name: "Language".into(), short_name: None, parameter_type: "string".into(),
                        description: "语言(C#/VB)".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: Some("CSharp".into()),
                        valid_values: Some(vec!["CSharp".into(),"VisualBasic".into()]) },
                    PsParameterSpec { name: "PassThru".into(), short_name: None, parameter_type: "switch".into(),
                        description: "返回类型对象".into(), mandatory: false, is_switch: true,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                ],
                alias_names: vec![],
                common_parameters: true,
            },
            PsCmdletSpec {
                name: "Try-Catch-Finally".into(), verb: "Try".into(), noun: "CatchFinally".into(),
                description: "异常处理结构".into(), module: Some("Microsoft.PowerShell.Core".into()),
                parameters: vec![],
                alias_names: vec![],
                common_parameters: false,
            },
            PsCmdletSpec {
                name: "Throw".into(), verb: "Throw".into(), noun: "".into(),
                description: "抛出异常".into(), module: Some("Microsoft.PowerShell.Core".into()),
                parameters: vec![
                    PsParameterSpec { name: "ExceptionMessage".into(), short_name: None, parameter_type: "string".into(),
                        description: "异常消息".into(), mandatory: false, is_switch: false,
                        accepts_pipeline_input: false, position: None, default_value: None, valid_values: None },
                ],
                alias_names: vec![],
                common_parameters: false,
            },
        ];

        for cmdlet in cmdlets {
            self.cmdlet_registry.insert(cmdlet.name.clone(), cmdlet.clone());

            for alias in &cmdlet.alias_names {
                if !alias.is_empty() {
                    self.cmdlet_registry.insert(alias.clone(), cmdlet.clone());
                }
            }
        }
    }

    fn get_cmdlet_name_suggestions(&self, prefix: &str) -> Vec<CompletionSuggestion> {
        let prefix_lower = prefix.to_lowercase();

        self.cmdlet_registry.values()
            .filter(|c| {
                prefix.is_empty()
                    || c.name.to_lowercase().contains(&prefix_lower)
                    || c.alias_names.iter().any(|a| a.to_lowercase().contains(&prefix_lower))
            })
            .map(|c| CompletionSuggestion {
                text: format!("{} ", c.name),
                display_text: c.name.clone(),
                description: c.description.clone(),
                kind: CompletionKind::Command,
                priority: c.popularity_score(),
                metadata: {
                    let mut m = HashMap::new();
                    if let Some(ref mod_name) = c.module {
                        m.insert("module".to_string(), mod_name.clone());
                    }
                    if !c.alias_names.is_empty() {
                        m.insert("aliases".to_string(), c.alias_names.join(", "));
                    }
                    m
                },
            })
            .collect()
    }

    fn get_alias_suggestions(&self, prefix: &str) -> Vec<CompletionSuggestion> {
        let known_aliases: Vec<(&str, &str, u8)> = vec![
            ("?", "Where-Object", 92),
            ("%", "ForEach-Object", 91),
            ("select", "Select-Object", 90),
            ("sort", "Sort-Object", 89),
            ("group", "Group-Object", 88),
            ("measure", "Measure-Object", 87),
            ("compare", "Compare-Object", 86),
            ("tee", "Tee-Object", 85),
            ("ft", "Format-Table", 84),
            ("fl", "Format-List", 83),
            ("fw", "Format-Wide", 82),
            ("gm", "Get-Member", 81),
            ("iex", "Invoke-Expression", 80),
            ("kill", "Stop-Process", 79),
            ("echo", "Write-Host", 78),
            ("cd", "Set-Location", 77),
            ("pwd", "Get-Location", 77),
            ("ls", "Get-ChildItem", 77),
            ("dir", "Get-ChildItem", 77),
            ("cat", "Get-Content", 76),
            ("cp", "Copy-Item", 76),
            ("mv", "Move-Item", 76),
            ("rm", "Remove-Item", 76),
            ("del", "Remove-Item", 76),
            ("pushd", "Push-Location", 75),
            ("popd", "Pop-Location", 75),
            ("sleep", "Start-Sleep", 74),
            ("help", "Get-Help", 73),
            ("man", "Get-Help", 73),
            ("ps", "Get-Process", 72),
            ("gps", "Get-Process", 72),
            ("gsv", "Get-Service", 71),
            ("join", "Join-Path", 70),
            ("split", "Split-Path", 69),
            ("new", "New-Object", 68),
            ("ren", "Rename-Item", 67),
            ("copy", "Copy-Item", 66),
            ("move", "Move-Item", 66),
            ("write", "Write-Host", 65),
            ("set", "Set-Variable", 64),
            ("gv", "Get-Variable", 63),
            ("sv", "Set-Variable", 62),
        ];

        known_aliases.into_iter()
            .filter(|(alias, _, _)| prefix.is_empty() || alias.starts_with(prefix))
            .map(|(alias, full_name, priority)| CompletionSuggestion {
                text: format!("{} ", alias),
                display_text: format!("{} ({})", alias, full_name),
                description: format!("{} 的别名", full_name),
                kind: CompletionKind::Alias,
                priority,
                metadata: {
                    let mut m = HashMap::new();
                    m.insert("expands_to".to_string(), full_name.to_string());
                    m
                },
            })
            .collect()
    }

    fn get_common_parameter_suggestions(&self, prefix: &str) -> Vec<CompletionSuggestion> {
        let common_params: Vec<(&str, &str, bool)> = vec![
            ("Verbose", "详细输出", true),
            ("Debug", "调试输出", true),
            ("ErrorAction", "错误行为", false),
            ("WarningAction", "警告行为", true),
            ("InformationAction", "信息行为", true),
            ("ErrorVariable", "错误变量", false),
            ("WarningVariable", "警告变量", false),
            ("InformationVariable", "信息变量", false),
            ("OutVariable", "输出变量", false),
            ("OutBuffer", "输出缓冲区", false),
            ("PipelineVariable", "管道变量", false),
            ("WhatIf", "模拟执行", true),
            ("Confirm", "确认提示", true),
        ];

        common_params.into_iter()
            .filter(|(name, _, _)| prefix.is_empty() || format!("-{}", name.to_lowercase()).starts_with(&prefix.to_lowercase()))
            .map(|(name, desc, is_switch)| CompletionSuggestion {
                text: if is_switch { format!("{} ", format!("-{}", name)) } else { format!("{} ", format!("-{}", name)) },
                display_text: format!("-{}", name),
                description: format!("[通用] {}", desc),
                kind: CompletionKind::Argument,
                priority: 70,
                metadata: {
                    let mut m = HashMap::new();
                    m.insert("common".to_string(), "true".to_string());
                    if is_switch { m.insert("switch".to_string(), "true".to_string()); }
                    m
                },
            })
            .collect()
    }

    fn get_variable_suggestions(&self, prefix: &str) -> Vec<CompletionSuggestion> {
        let built_in_vars: Vec<(&str, &str, u8)> = vec![
            ("$", "自动变量(最后一个)", 95),
            ("$?", "上一个操作状态", 94),
            ("$_", "管道当前对象", 93),
            ("$Args", "脚本参数", 92),
            ("$HOME", "用户主目录", 91),
            ("$PWD", "当前工作目录", 90),
            ("$env:", "环境变量前缀", 89),
            ("$PSVersionTable", "PS版本信息", 88),
            ("$null", "空值", 87),
            ("$true/$false", "布尔值", 86),
            ("$this", "当前对象", 85),
            ("$ExecutionContext", "执行上下文", 84),
            ("$Host", "主机应用", 83),
            ("$LastExitCode", "退出码", 82),
            ("$Matches", "正则匹配结果", 81),
            ("$MyInvocation", "调用信息", 80),
            ("$NestedPromptLevel", "嵌套层级", 79),
            ("$Profile", "配置文件路径", 78),
            ("$PSScriptRoot", "脚本根目录", 77),
            ("$PSCommandPath", "脚本完整路径", 76),
            ("$ShellId", "Shell标识符", 75),
            ("$StackTrace", "堆栈跟踪", 74),
            ("$OFS", "输出字段分隔符", 73),
            ("$FormatEnumerationLimit", "枚举限制", 72),
            ("$MaximumVariableCount", "最大变量数", 71),
            ("$Error", "错误数组", 70),
            ("$ErrorView", "错误视图", 69),
            ("$ErrorActionPreference", "默认错误行为", 68),
            ("$VerbosePreference", "默认详细行为", 67),
            ("$WhatIfPreference", "默认模拟行为", 66),
            ("$ConfirmPreference", "默认确认行为", 65),
            ("$DebugPreference", "默认调试行为", 64),
            ("$InformationPreference", "默认信息行为", 63),
            ("$ProgressPreference", "默认进度行为", 62),
            ("$WarningPreference", "默认警告行为", 61),
            ("$OutputEncoding", "输出编码", 60),
            ("$MaximumHistoryCount", "历史记录上限", 59),
            ("$MaximumFunctionCount", "函数数量上限", 58),
        ];

        built_in_vars.into_iter()
            .filter(|(var, _, _)| prefix.is_empty() || var.to_lowercase().starts_with(&prefix.to_lowercase()))
            .map(|(var, desc, priority)| CompletionSuggestion {
                text: var.to_string(),
                display_text: var.to_string(),
                description: desc.to_string(),
                kind: CompletionKind::Variable,
                priority,
                metadata: HashMap::new(),
            })
            .collect()
    }

    fn get_gci_parameter_suggestions(&self, prefix: &str) -> Vec<CompletionSuggestion> {
        let params = [
            ("-Path", "路径", false),
            ("-Filter", "过滤器", false),
            ("-Recurse", "递归", true),
            ("-Force", "包含隐藏文件", true),
            ("-Name", "仅名称", true),
            ("-File", "仅文件", true),
            ("-Directory", "仅目录", true),
            ("-Hidden", "仅隐藏项", true),
            ("-Depth", "递归深度", false),
        ];
        params.into_iter()
            .filter(|(name, _, _)| prefix.is_empty() || name.to_lowercase().starts_with(&prefix.to_lowercase()))
            .map(|(name, desc, is_sw)| CompletionSuggestion {
                text: if is_sw { format!("{} ", name) } else { format!("{} ", name) },
                display_text: name.to_string(),
                description: desc.to_string(),
                kind: CompletionKind::Argument,
                priority: 85,
                metadata: HashMap::new(),
            })
            .collect()
    }

    fn get_gc_parameter_suggestions(&self, prefix: &str) -> Vec<CompletionSuggestion> {
        let params = [
            ("-Path", "文件路径", false),
            ("-TotalCount", "读取行数", false),
            ("-Tail", "最后N行", false),
            ("-Encoding", "编码", false),
            ("-Raw", "原始模式", true),
            ("-Wait", "持续监听", true),
        ];
        params.into_iter()
            .filter(|(name, _, _)| prefix.is_empty() || name.to_lowercase().starts_with(&prefix.to_lowercase()))
            .map(|(name, desc, is_sw)| CompletionSuggestion {
                text: if is_sw { format!("{} ", name) } else { format!("{} ", name) },
                display_text: name.to_string(),
                description: desc.to_string(),
                kind: CompletionKind::Argument,
                priority: 84,
                metadata: HashMap::new(),
            })
            .collect()
    }

    fn get_where_object_suggestions(&self, prefix: &str) -> Vec<CompletionSuggestion> {
        let params = [
            ("-FilterScript", "筛选脚本块 {$_.Prop -eq ...}", false),
            ("-Property", "属性名", false),
            ("-EQ", "等于", true),
            ("-NE", "不等于", true),
            ("-GT", "大于", true),
            ("-LT", "小于", true),
            ("-Like", "通配匹配", true),
            ("-Match", "正则匹配", true),
            ("-Contains", "包含", true),
            ("-Not", "取反", true),
            ("-CEQ", "区分大小写等于", true),
            ("-CLike", "区分大小写通配", true),
            ("-CMatch", "区分大小写正则", true),
        ];
        params.into_iter()
            .filter(|(name, _, _)| prefix.is_empty() || name.to_lowercase().starts_with(&prefix.to_lowercase()))
            .map(|(name, desc, is_sw)| CompletionSuggestion {
                text: if is_sw { format!("{} ", name) } else { format!("{} ", name) },
                display_text: name.to_string(),
                description: desc.to_string(),
                kind: CompletionKind::Argument,
                priority: 86,
                metadata: HashMap::new(),
            })
            .collect()
    }

    fn get_foreach_object_suggestions(&self, prefix: &str) -> Vec<CompletionSuggestion> {
        let params = [
            ("-Process", "处理脚本块 { $_ | ... }", false),
            ("-Begin", "前置脚本块", false),
            ("-End", "后置脚本块", false),
            ("-MemberName", "方法/属性名", false),
            ("-InputObject", "输入对象", false),
        ];
        params.into_iter()
            .filter(|(name, _, _)| prefix.is_empty() || name.to_lowercase().starts_with(&prefix.to_lowercase()))
            .map(|(name, desc, is_sw)| CompletionSuggestion {
                text: if is_sw { format!("{} ", name) } else { format!("{} ", name) },
                display_text: name.to_string(),
                description: desc.to_string(),
                kind: CompletionKind::Argument,
                priority: 85,
                metadata: HashMap::new(),
            })
            .collect()
    }

    fn get_import_module_suggestions(&self, prefix: &str) -> Vec<CompletionSuggestion> {
        let modules: Vec<(&str, &str)> = vec![
            ("Microsoft.PowerShell.Management", "核心管理"),
            ("Microsoft.PowerShell.Utility", "工具函数"),
            ("Microsoft.PowerShell.Core", "核心引擎"),
            ("Microsoft.PowerShell.Security", "安全相关"),
            ("Microsoft.WSMan.Management", "WS-Man管理"),
            ("Microsoft.PowerShell.Diagnostics", "诊断日志"),
            ("ISE", "集成脚本环境"),
            ("PackageManagement", "包管理"),
            ("PowerShellGet", "模块获取"),
            ("PSReadLine", "命令行编辑增强"),
            ("PSScriptAnalyzer", "脚本分析"),
            ("Az.Accounts", "Azure身份验证"),
            ("Az.Compute", "Azure计算"),
            ("Az.Storage", "Azure存储"),
            ("Az.Network", "Azure网络"),
            ("Docker", "容器管理"),
            ("Posh-Git", "Git集成"),
            ("oh-my-posh", "主题美化"),
            ("z", "目录跳转"),
            ("PSColors", "颜色输出"),
        ];

        modules.into_iter()
            .filter(|(name, _)| prefix.is_empty() || name.to_lowercase().contains(&prefix.to_lowercase()))
            .map(|(name, desc)| CompletionSuggestion {
                text: format!("{} ", name),
                display_text: name.to_string(),
                description: desc.to_string(),
                kind: CompletionKind::Argument,
                priority: 82,
                metadata: HashMap::new(),
            })
            .collect()
    }

    fn get_pipeline_cmdlet_suggestions(&self, prefix: &str) -> Vec<CompletionSuggestion> {
        let pipeline_friendly: Vec<(&str, &str, u8)> = vec![
            ("Where-Object", "筛选对象", 95),
            ("ForEach-Object", "遍历处理", 94),
            ("Select-Object", "选择属性", 93),
            ("Sort-Object", "排序", 92),
            ("Group-Object", "分组", 91),
            ("Measure-Object", "统计", 90),
            ("Compare-Object", "比较", 89),
            ("Tee-Object", "分叉输出", 88),
            ("Format-Table", "表格格式", 87),
            ("Format-List", "列表格式", 86),
            ("Format-Wide", "宽屏格式", 85),
            ("Out-File", "输出到文件", 84),
            ("Out-String", "转为字符串", 83),
            ("Out-Null", "丢弃输出", 82),
            ("Get-Member", "查看成员", 81),
            ("ConvertTo-Json", "转JSON", 80),
            ("ConvertFrom-Json", "从JSON解析", 79),
            ("ConvertTo-Csv", "转CSV", 78),
            ("Add-Member", "添加成员", 77),
            (" Tee-Object", "保存并传递", 76),
        ];

        pipeline_friendly.into_iter()
            .filter(|(name, _, _)| prefix.is_empty() || name.to_lowercase().contains(&prefix.to_lowercase()))
            .map(|(name, desc, priority)| CompletionSuggestion {
                text: format!("{} ", name),
                display_text: name.to_string(),
                description: format!("[管道] {}", desc),
                kind: CompletionKind::Command,
                priority,
                metadata: {
                    let mut m = HashMap::new();
                    m.insert("pipeline_friendly".to_string(), "true".to_string());
                    m
                },
            })
            .collect()
    }

    fn get_parameter_suggestions_for_cmdlet(&self, spec: &PsCmdletSpec, prefix: &str) -> Vec<CompletionSuggestion> {
        spec.parameters.iter()
            .filter(|p| prefix.is_empty() || format!("-{}", p.name.to_lowercase()).starts_with(&prefix.to_lowercase()))
            .map(|p| CompletionSuggestion {
                text: if p.is_switch { format!("{} ", format!("-{}", p.name)) } else { format!("{} ", format!("-{}", p.name)) },
                display_text: format!("-{}", p.name),
                description: if p.mandatory { format!("[必需] {}", p.description) } else { p.description.clone() },
                kind: CompletionKind::Argument,
                priority: if p.mandatory { 95 } else { 80 },
                metadata: {
                    let mut m = HashMap::new();
                    m.insert("type".to_string(), p.parameter_type.clone());
                    if p.mandatory { m.insert("mandatory".to_string(), "true".to_string()); }
                    if p.is_switch { m.insert("switch".to_string(), "true".to_string()); }
                    if let Some(ref vals) = p.valid_values {
                        m.insert("valid_values".to_string(), vals.join(", "));
                    }
                    m
                },
            })
            .collect()
    }
}

impl PsCmdletSpec {
    fn popularity_score(&self) -> u8 {
        match self.verb.as_str() {
            "Get" => 95,
            "Set" => 90,
            "New" | "Remove" => 88,
            "Write" | "Add" => 85,
            "Import" | "Export" => 82,
            "Invoke" | "Start" | "Stop" => 80,
            "Where" | "ForEach" | "Select" | "Sort" => 92,
            "Format" | "Out" | "ConvertTo" | "ConvertFrom" => 78,
            "Test" | "Compare" | "Measure" => 75,
            _ => 70,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_cmdlet() {
        let parser = PowerShellParser::new();
        let result = parser.parse("Get-ChildItem -Path C:\\", 20);

        assert!(result.errors.is_empty());
        match result.ast {
            PsAstNode::Cmdlet(cmd) => {
                assert_eq!(cmd.name, "Get-ChildItem");
            }
            _ => panic!("Expected Cmdlet node"),
        }
    }

    #[test]
    fn test_parse_pipeline() {
        let parser = PowerShellParser::new();
        let result = parser.parse("Get-Process | Where-Object {$_.CPU -gt 100}", 40);

        match result.ast {
            PsAstNode::StatementList(stmts) | PsAstNode::Pipeline(_) => {
                assert!(result.cursor_context.in_pipeline);
            }
            _ => {}
        }
    }

    #[test]
    fn test_parse_variable() {
        let parser = PowerShellParser::new();
        let result = parser.parse("$env:PATH", 5);

        assert!(result.cursor_context.in_variable_expansion);
    }

    #[test]
    fn test_tokenize_ps_syntax() {
        let parser = PowerShellParser::new();
        let tokens = parser.tokenize("Get-ChildItem -Recurse -Force | Select-Object -First 10");

        let has_cmdlet = tokens.iter().any(|t| matches!(t, PsToken::Cmdlet(c) if c == "Get-ChildItem"));
        let has_pipe = tokens.iter().any(|t| t == PsToken::Pipe);
        let has_switch_recurse = tokens.iter().any(|t| matches!(t, PsToken::Parameter(p) if p == "-Recurse"));

        assert!(has_cmdlet);
        assert!(has_pipe);
        assert!(has_switch_recurse);
    }

    #[test]
    fn test_cmdlet_registry_size() {
        let parser = PowerShellParser::new();
        assert!(parser.cmdlet_registry.len() >= 50,
            "Expected >=50 cmdlets, got {}", parser.cmdlet_registry.len());
    }

    #[test]
    fn test_get_child_item_has_params() {
        let parser = PowerShellParser::new();
        let gci = parser.cmdlet_registry.get("Get-ChildItem").unwrap();
        assert!(!gci.parameters.is_empty());
        assert!(gci.parameters.iter().any(|p| p.name == "Recurse"));
        assert!(gci.parameters.iter().any(|p| p.name == "Force"));
        assert!(gci.parameters.iter().any(|p| p.name == "Path"));
    }

    #[test]
    fn test_alias_resolution() {
        let parser = PowerShellParser::new();
        assert!(parser.cmdlet_registry.contains_key("gci"));
        assert!(parser.cmdlet_registry.contains_key("ls"));
        assert!(parser.cmdlet_registry.contains_key("?"));
        assert!(parser.cmdlet_registry.contains_key("%"));
    }

    #[test]
    fn test_completions_for_get_childitem() {
        let parser = PowerShellParser::new();
        let ctx = CompletionContext::new("Get-ChildItem -", 15);
        let suggestions = parser.get_suggestions(&ctx);

        assert!(!suggestions.is_empty());
        assert!(suggestions.iter().any(|s| s.text.contains("-Recurse")));
        assert!(suggestions.iter().any(|s| s.text.contains("-Force")));
    }

    #[test]
    fn test_completions_for_where_object() {
        let parser = PowerShellParser::new();
        let ctx = CompletionContext::new("Where-Object -", 13);
        let suggestions = parser.get_suggestions(&ctx);

        assert!(!suggestions.is_empty());
        assert!(suggestions.iter().any(|s| s.text.contains("-EQ") || s.text.contains("-Property")));
    }

    #[test]
    fn test_variable_completions() {
        let parser = PowerShellParser::new();
        let ctx = CompletionContext::new("$", 1);
        let suggestions = parser.get_suggestions(&ctx);

        assert!(!suggestions.is_empty());
        assert!(suggestions.iter().any(|s| s.text.contains("$HOME")));
        assert!(suggestions.iter().any(|s| s.text.contains("$_")));
    }

    #[test]
    fn test_import_module_completions() {
        let parser = PowerShellParser::new();
        let ctx = CompletionContext::new("Import-Module ", 14);
        let suggestions = parser.get_suggestions(&ctx);

        assert!(!suggestions.is_empty());
        assert!(suggestions.iter().any(|s| s.display_text.contains("PowerShell")));
    }

    #[test]
    fn test_pipeline_context_detection() {
        let parser = PowerShellParser::new();
        let result = parser.parse("Get-Process | ", 16);

        assert!(result.cursor_context.in_pipeline);
        assert_eq!(result.cursor_context.pipeline_position, Some(1));
    }

    #[test]
    fn test_common_parameters() {
        let parser = PowerShellParser::new();
        let ctx = CompletionContext::new("Write-Hello -V", 12);
        let suggestions = parser.get_suggestions(&ctx);

        assert!(suggestions.iter().any(|s| s.text.contains("-Verbose")));
    }

    #[test]
    fn test_empty_input() {
        let parser = PowerShellParser::new();
        let result = parser.parse("", 0);

        assert!(matches!(result.ast, PsAstNode::Empty));
    }

    #[test]
    fn test_string_literal_tokenization() {
        let parser = PowerShellParser::new();
        let tokens = parser.tokenize("\"hello world\" 'single quoted'");

        assert_eq!(tokens.len(), 2);
    }

    #[test]
    fn test_module_info_structure() {
        let gci = PowerShellParser::new().cmdlet_registry.get("Get-ChildItem").unwrap();
        assert_eq!(gci.module.as_deref(), Some("Microsoft.PowerShell.Management"));
        assert!(!gci.alias_names.is_empty());
    }
}
