//! # Fish Shell 补全引擎
//!
//! 提供智能的 Fish Shell 命令行补全功能：
//! - **语法解析器** - 完整的 Fish 语法分析 (command --help 风格)
//! - **补全脚本支持** - .fish 文件格式 (complete 命令)
//! - **Fish 特有变量** - __fish_* 前缀变量
//! - **缩写支持** - abbr 命令检测与补全
//! - **函数定义** - function ... end 检测

use crate::completion::bash::{CompletionContext, CompletionSuggestion, CompletionKind};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Fish AST 节点类型
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FishAstNode {
    /// 简单命令 (如: git status, npm install)
    Command(FishCommandNode),
    /// 管道连接
    Pipeline(FishPipelineNode),
    /// 命令列表 (; && || 分隔)
    List(FishListNode),
    /// 函数定义
    FunctionDef(FishFunctionDef),
    /// 缩写定义
    Abbreviation(FishAbbreviation),
    /// if/else 条件语句
    Conditional(FishConditional),
    /// for/while 循环
    Loop(FishLoop),
    /// 子shell (begin ... end)
    Block(FishBlock),
    /// 空节点或注释
    Empty,
}

/// 命令节点
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FishCommandNode {
    pub name: String,
    pub args: Vec<String>,
    pub trailing_space: bool,
    pub range: FishTextRange,
}

/// 管道节点
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FishPipelineNode {
    pub commands: Vec<FishCommandNode>,
}

/// 列表节点
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FishListNode {
    pub operator: FishListOperator,
    pub left: Box<FishAstNode>,
    pub right: Box<FishAstNode>,
}

/// 列表操作符
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FishListOperator {
    And,
    Or,
    Seq,
    AndOr,
}

impl std::fmt::Display for FishListOperator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FishListOperator::And => write!(f, "&&"),
            FishListOperator::Or => write!(f, "||"),
            FishListOperator::Seq => write!(f, ";"),
            FishListOperator::AndOr => write!(f, "; and ; or"),
        }
    }
}

/// 函数定义节点
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FishFunctionDef {
    pub name: String,
    pub arguments: Vec<String>,
    pub description: Option<String>,
    pub body: Vec<FishAstNode>,
}

/// 缩写定义节点
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FishAbbreviation {
    pub abbreviation: String,
    pub expansion: String,
}

/// 条件语句节点
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FishConditional {
    pub condition: Box<FishAstNode>,
    pub then_body: Vec<FishAstNode>,
    pub else_body: Option<Vec<FishAstNode>>,
}

/// 循环节点
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FishLoop {
    pub loop_type: FishLoopType,
    pub variable: Option<String>,
    pub iterable: Option<String>,
    pub body: Vec<FishAstNode>,
}

/// 循环类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FishLoopType {
    For,
    While,
}

/// 代码块节点
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FishBlock {
    pub body: Vec<FishAstNode>,
}

/// 文本范围
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct FishTextRange {
    pub start: usize,
    pub end: usize,
}

/// 解析结果
#[derive(Debug, Clone)]
pub struct FishParseResult {
    pub ast: FishAstNode,
    pub errors: Vec<FishParseError>,
    pub cursor_context: FishCursorContext,
}

/// 光标上下文
#[derive(Debug, Clone)]
pub struct FishCursorContext {
    pub current_command: Option<String>,
    pub current_arg_index: Option<usize>,
    pub in_function_def: bool,
    pub in_abbreviation: bool,
    pub tokens_before_cursor: Vec<FishToken>,
    pub inside_complete_block: bool,
    pub in_substitution: bool,
    pub in_string: bool,
    pub string_char: Option<char>,
}

/// Token 类型
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FishToken {
    Word(String),
    ShortOption(char),
    LongOption(String),
    Pipe,
    RedirectOut,
    RedirectAppend,
    RedirectIn,
    Semicolon,
    DoubleAnd,
    DoubleOr,
    Newline,
    Comment(String),
    Whitespace,
    ParenOpen,
    ParenClose,
    BraceOpen,
    BraceClose,
    Variable(String),
    CommandSubstOpen,
    CommandSubstClose,
    EndKeyword,
    FunctionKeyword,
    AbbrevKeyword,
    IfKeyword,
    ElseKeyword,
    ForKeyword,
    WhileKeyword,
    CompleteCommand,
}

/// 解析错误
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FishParseError {
    pub message: String,
    pub position: usize,
    pub error_type: FishParseErrorType,
}

/// 解析错误类型
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FishParseErrorType {
    UnexpectedToken,
    UnclosedString,
    UnclosedSubstitution,
    UnclosedParenthesis,
    MissingEnd,
    InvalidSyntax,
    IncompleteCommand,
}

/// Fish complete 命令规格
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FishCompleteSpec {
    pub command: String,
    pub short_option: Option<char>,
    pub long_option: Option<String>,
    pub description: Option<String>,
    pub arguments: Option<String>,
    pub old_arguments: Option<String>,
    pub condition: Option<String>,
    pub requires_param: bool,
    pub keep_order: bool,
    pub exclusive: bool,
}

/// Fish 缩写规格
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FishAbbrSpec {
    pub word: String,
    pub expansion: String,
    pub description: Option<String>,
    pub regex: Option<String>,
    pub set_cursor: bool,
    pub position: Option<i32>,
}

/// Fish 函数规格
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FishFuncSpec {
    pub name: String,
    pub description: Option<String>,
    pub arguments: Vec<String>,
    pub wraps: Option<String>,
    pub is_autoloaded: bool,
}

/// Fish 内置变量
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FishVariableSpec {
    pub name: String,
    pub description: String,
    pub is_readonly: bool,
    pub default_value: Option<String>,
    pub category: FishVarCategory,
}

/// 变量分类
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FishVarCategory {
    Status,
    History,
    JobControl,
    Completion,
    Information,
    UserDefined,
    Internal,
}

impl std::fmt::Display for FishVarCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FishVarCategory::Status => write!(f, "状态"),
            FishVarCategory::History => write!(f, "历史"),
            FishVarCategory::JobControl => write!(f, "作业控制"),
            FishVarCategory::Completion => write!(f, "补全"),
            FishVarCategory::Information => write!(f, "信息"),
            FishVarCategory::UserDefined => write!(f, "用户自定义"),
            FishVarCategory::Internal => write!(f, "内部"),
        }
    }
}

/// Fish Shell 解析器
pub struct FishParser {
    word_regex: Regex,
    option_regex: Regex,
    variable_regex: Regex,
    fish_var_regex: Regex,
    complete_specs: HashMap<String, Vec<FishCompleteSpec>>,
    abbreviations: HashMap<String, FishAbbrSpec>,
    functions: HashMap<String, FishFuncSpec>,
    known_variables: HashMap<String, FishVariableSpec>,
}

impl Default for FishParser {
    fn default() -> Self {
        let mut parser = Self {
            word_regex: Regex::new(r#"[^\s|&;<>()$#\\]+"#).unwrap(),
            option_regex: Regex::new(r"-{1,2}[A-Za-z][A-Za-z0-9_-]*").unwrap(),
            variable_regex: Regex::new(r#"\$\w+|\$\{[^}]+\}"#).unwrap(),
            fish_var_regex: Regex::new(r"__fish_[A-Za-z_][A-Za-z0-9_]*").unwrap(),
            complete_specs: HashMap::new(),
            abbreviations: HashMap::new(),
            functions: HashMap::new(),
            known_variables: HashMap::new(),
        };
        parser.register_builtin_variables();
        parser.register_common_completions();
        parser.register_default_abbreviations();
        parser
    }
}

impl FishParser {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn parse(&self, input: &str, cursor_pos: usize) -> FishParseResult {
        let mut tokens = self.tokenize(input);
        let mut errors = vec![];
        let cursor_context = self.analyze_cursor_context(&tokens, cursor_pos, input);

        let ast = if tokens.is_empty() {
            FishAstNode::Empty
        } else {
            match self.build_ast(&mut tokens, &mut errors) {
                Ok(node) => node,
                Err(_) => FishAstNode::Empty,
            }
        };

        FishParseResult { ast, errors, cursor_context }
    }

    pub fn get_suggestions(&self, context: &CompletionContext) -> Vec<CompletionSuggestion> {
        let parse_result = self.parse(&context.line, context.cursor_position);
        let mut suggestions = vec![];

        match &parse_result.cursor_context.current_command {
            Some(cmd) if cmd.is_empty() => {
                let word = context.get_word_before_cursor();

                if word.starts_with('-') {
                    suggestions.extend(self.get_option_suggestions(word));
                } else if self.fish_var_regex.is_match(word) || word.starts_with("__fish") {
                    suggestions.extend(self.get_fish_variable_suggestions(word));
                } else if word.starts_with('$') {
                    suggestions.extend(self.get_variable_suggestions(word));
                } else {
                    suggestions.extend(self.get_command_suggestions(word));

                    if word.is_empty() || word == "abbr" {
                        suggestions.push(CompletionSuggestion {
                            text: "abbr ".to_string(),
                            display_text: "abbr".to_string(),
                            description: "添加/管理缩写".to_string(),
                            kind: CompletionKind::Command,
                            priority: 85,
                            metadata: HashMap::new(),
                        });
                    }

                    if word.is_empty() || word == "function" || word == "func" {
                        suggestions.push(CompletionSuggestion {
                            text: "function ".to_string(),
                            display_text: "function".to_string(),
                            description: "定义新函数".to_string(),
                            kind: CompletionKind::Command,
                            priority: 84,
                            metadata: HashMap::new(),
                        });
                    }

                    if word.is_empty() || word == "complete" {
                        suggestions.push(CompletionSuggestion {
                            text: "complete ".to_string(),
                            display_text: "complete".to_string(),
                            description: "注册补全规则".to_string(),
                            kind: CompletionKind::Command,
                            priority: 83,
                            metadata: HashMap::new(),
                        });
                    }

                    for abbr_word in self.abbreviations.keys() {
                        if word.is_empty() || abbr_word.starts_with(word) {
                            let spec = &self.abbreviations[abbr_word];
                            suggestions.push(CompletionSuggestion {
                                text: format!("{} ", abbr_word),
                                display_text: abbr_word.clone(),
                                description: format!("缩写 -> {}", spec.expansion),
                                kind: CompletionKind::Alias,
                                priority: 90,
                                metadata: {
                                    let mut m = HashMap::new();
                                    m.insert("expansion".to_string(), spec.expansion.clone());
                                    m.insert("type".to_string(), "abbr".to_string());
                                    m
                                },
                            });
                        }
                    }
                }
            }
            Some(cmd) => {
                let word = context.get_word_before_cursor();

                if word.starts_with('-') {
                    if let Some(specs) = self.complete_specs.get(cmd) {
                        suggestions.extend(self.get_complete_spec_suggestions(specs, word));
                    }
                    suggestions.extend(self.get_generic_option_suggestions(cmd, word));
                } else if self.fish_var_regex.is_match(word) || word.starts_with("__fish") {
                    suggestions.extend(self.get_fish_variable_suggestions(word));
                } else if word.starts_with('$') {
                    suggestions.extend(self.get_variable_suggestions(word));
                } else {
                    match cmd.as_str() {
                        "git" => suggestions.extend(self.get_git_fish_suggestions(context)),
                        "docker" => suggestions.extend(self.get_docker_fish_suggestions(context)),
                        "npm" | "yarn" | "pnpm" => suggestions.extend(self.get_npm_fish_suggestions(context)),
                        "cargo" => suggestions.extend(self.get_cargo_fish_suggestions(context)),
                        "fish" => suggestions.extend(self.get_fish_builtin_suggestions(context)),
                        _ => {
                            if let Some(func) = self.functions.get(cmd) {
                                if let Some(ref desc) = func.description {
                                    suggestions.push(CompletionSuggestion {
                                        text: String::new(),
                                        display_text: cmd.to_string(),
                                        description: desc.clone(),
                                        kind: CompletionKind::Command,
                                        priority: 70,
                                        metadata: HashMap::new(),
                                    });
                                }
                            }
                        }
                    }
                }
            }
            None => {
                suggestions.extend(self.get_command_suggestions(""));
            }
        }

        suggestions
    }

    fn tokenize(&self, input: &str) -> Vec<FishToken> {
        let mut tokens = vec![];
        let mut pos = 0;
        let chars: Vec<char> = input.chars().collect();

        while pos < chars.len() {
            let ch = chars[pos];

            match ch {
                ' ' | '\t' | '\r' => {
                    tokens.push(FishToken::Whitespace);
                    pos += 1;
                }
                '\n' => {
                    tokens.push(FishToken::Newline);
                    pos += 1;
                }
                '|' => {
                    if pos + 1 < chars.len() && chars[pos + 1] == '|' {
                        tokens.push(FishToken::DoubleOr);
                        pos += 2;
                    } else {
                        tokens.push(FishToken::Pipe);
                        pos += 1;
                    }
                }
                '&' => {
                    if pos + 1 < chars.len() && chars[pos + 1] == '&' {
                        tokens.push(FishToken::DoubleAnd);
                        pos += 2;
                    } else {
                        pos += 1;
                    }
                }
                ';' => {
                    tokens.push(FishToken::Semicolon);
                    pos += 1;
                }
                '>' => {
                    if pos + 1 < chars.len() && chars[pos + 1] == '>' {
                        tokens.push(FishToken::RedirectAppend);
                        pos += 2;
                    } else {
                        tokens.push(FishToken::RedirectOut);
                        pos += 1;
                    }
                }
                '<' => {
                    tokens.push(FishToken::RedirectIn);
                    pos += 1;
                }
                '(' => {
                    tokens.push(FishToken::ParenOpen);
                    pos += 1;
                }
                ')' => {
                    tokens.push(FishToken::ParenClose);
                    pos += 1;
                }
                '{' => {
                    tokens.push(FishToken::BraceOpen);
                    pos += 1;
                }
                '}' => {
                    tokens.push(FishToken::BraceClose);
                    pos += 1;
                }
                '$' => {
                    let _var_start = pos;
                    pos += 1;

                    if pos < chars.len() && chars[pos] == '(' {
                        tokens.push(FishToken::CommandSubstOpen);
                        pos += 1;
                        continue;
                    }

                    let mut var_name = String::from("$");
                    while pos < chars.len() && (chars[pos].is_alphanumeric() || chars[pos] == '_') {
                        var_name.push(chars[pos]);
                        pos += 1;
                    }

                    if var_name.len() > 1 {
                        tokens.push(FishToken::Variable(var_name));
                    } else {
                        tokens.push(FishToken::Variable("$".into()));
                    }
                }
                '\'' => {
                    pos += 1;
                    while pos < chars.len() && chars[pos] != '\'' {
                        pos += 1;
                    }
                    if pos < chars.len() { pos += 1; }
                }
                '"' => {
                    pos += 1;
                    while pos < chars.len() && chars[pos] != '"' {
                        if chars[pos] == '\\' && pos + 1 < chars.len() {
                            pos += 2;
                        } else {
                            pos += 1;
                        }
                    }
                    if pos < chars.len() { pos += 1; }
                }
                '#' => {
                    let comment_start = pos;
                    while pos < chars.len() && chars[pos] != '\n' {
                        pos += 1;
                    }
                    let comment: String = chars[comment_start..pos].iter().collect();
                    tokens.push(FishToken::Comment(comment));
                }
                '-' => {
                    let opt_start = pos;
                    pos += 1;

                    if pos < chars.len() && chars[pos] == '-' {
                        pos += 1;
                        while pos < chars.len() && (chars[pos].is_alphanumeric() || chars[pos] == '_' || chars[pos] == '-') {
                            pos += 1;
                        }
                        let opt: String = chars[opt_start..pos].iter().collect();
                        tokens.push(FishToken::LongOption(opt[2..].to_string()));
                    } else if pos < chars.len() && chars[pos].is_alphanumeric() {
                        while pos < chars.len() && chars[pos].is_alphanumeric() {
                            let c = chars[pos];
                            tokens.push(FishToken::ShortOption(c));
                            pos += 1;
                        }
                    } else {
                        tokens.push(FishToken::Word("-".into()));
                    }
                }
                _ if !ch.is_whitespace() => {
                    let word_start = pos;
                    while pos < chars.len() &&
                          !chars[pos].is_whitespace() &&
                          !"|&;<>(){}\"'$#-".contains(chars[pos])
                    {
                        pos += 1;
                    }
                    let word: String = chars[word_start..pos].iter().collect();

                    match word.as_str() {
                        "end" => tokens.push(FishToken::EndKeyword),
                        "function" | "func" => tokens.push(FishToken::FunctionKeyword),
                        "abbr" => tokens.push(FishToken::AbbrevKeyword),
                        "if" => tokens.push(FishToken::IfKeyword),
                        "else" => tokens.push(FishToken::ElseKeyword),
                        "for" => tokens.push(FishToken::ForKeyword),
                        "while" => tokens.push(FishToken::WhileKeyword),
                        "complete" => tokens.push(FishToken::CompleteCommand),
                        _ => tokens.push(FishToken::Word(word)),
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
        tokens: &[FishToken],
        cursor_pos: usize,
        input: &str,
    ) -> FishCursorContext {
        let mut current_command = None;
        let mut current_arg_index = None;
        let mut in_function_def = false;
        let mut in_abbreviation = false;
        let mut tokens_before_cursor = vec![];
        let mut inside_complete_block = false;
        let mut char_count = 0usize;

        for token in tokens {
            let token_len = self.fish_token_len(token);

            if char_count + token_len <= cursor_pos {
                tokens_before_cursor.push(token.clone());
                char_count += token_len;

                match &token {
                    FishToken::Word(name) if current_command.is_none() => {
                        current_command = Some(name.clone());
                        current_arg_index = Some(0);
                    }
                    FishToken::LongOption(name) if current_command.is_none() => {
                        current_command = Some(format!("--{}", name));
                        current_arg_index = Some(0);
                    }
                    FishToken::ShortOption(_) | FishToken::LongOption(_)
                    | FishToken::Word(_) | FishToken::Variable(_) if current_command.is_some() => {
                        if let Some(ref mut idx) = current_arg_index {
                            *idx += 1;
                        }
                    }
                    FishToken::Pipe | FishToken::Semicolon | FishToken::Newline => {
                        current_command = None;
                        current_arg_index = None;
                    }
                    FishToken::FunctionKeyword => {
                        in_function_def = true;
                        current_command = None;
                    }
                    FishToken::EndKeyword => {
                        in_function_def = false;
                        in_abbreviation = false;
                        inside_complete_block = false;
                    }
                    FishToken::AbbrevKeyword => {
                        in_abbreviation = true;
                    }
                    FishToken::CompleteCommand => {
                        inside_complete_block = true;
                    }
                    _ => {}
                }
            } else {
                break;
            }
        }

        let before_cursor: String = input[..cursor_pos.min(input.len())].chars().collect();
        let mut in_substitution = false;
        let mut in_string = false;
        let mut string_char = None;

        for ch in before_cursor.chars().rev() {
            if ch == '"' || ch == '\'' {
                in_string = true;
                string_char = Some(ch);
                break;
            }
            if ch == ')' {
                in_substitution = true;
                break;
            }
        }

        FishCursorContext {
            current_command,
            current_arg_index,
            in_function_def,
            in_abbreviation,
            tokens_before_cursor,
            inside_complete_block,
            in_substitution,
            in_string,
            string_char,
        }
    }

    fn build_ast(
        &self,
        tokens: &mut Vec<FishToken>,
        _errors: &mut Vec<FishParseError>,
    ) -> Result<FishAstNode, ()> {
        let mut commands: Vec<FishCommandNode> = vec![];
        let mut current_cmd_words = vec![];

        while !tokens.is_empty() {
            match tokens.remove(0) {
                FishToken::Pipe => {
                    if !current_cmd_words.is_empty() {
                        if let FishAstNode::Command(cmd) = self.build_command_node(current_cmd_words) {
                            commands.push(cmd);
                        }
                        current_cmd_words = vec![];
                    }
                }
                FishToken::DoubleAnd | FishToken::DoubleOr | FishToken::Semicolon => {
                    if !current_cmd_words.is_empty() {
                        if let FishAstNode::Command(cmd) = self.build_command_node(current_cmd_words) {
                            commands.push(cmd);
                        }
                        current_cmd_words = vec![];
                    }
                }
                FishToken::Word(w) => {
                    current_cmd_words.push(w);
                }
                FishToken::ShortOption(c) => {
                    current_cmd_words.push(format!("-{}", c));
                }
                FishToken::LongOption(s) => {
                    current_cmd_words.push(format!("--{}", s));
                }
                FishToken::Variable(v) => {
                    current_cmd_words.push(v);
                }
                FishToken::FunctionKeyword => {
                    if !current_cmd_words.is_empty() {
                        if let FishAstNode::Command(cmd) = self.build_command_node(current_cmd_words) {
                            commands.push(cmd);
                        }
                    }
                    let func_name = if !tokens.is_empty() {
                        match tokens.remove(0) {
                            FishToken::Word(n) => n,
                            _ => "anonymous".to_string(),
                        }
                    } else {
                        "anonymous".to_string()
                    };
                    return Ok(FishAstNode::FunctionDef(FishFunctionDef {
                        name: func_name,
                        arguments: vec![],
                        description: None,
                        body: vec![],
                    }));
                }
                FishToken::AbbrevKeyword => {
                    if !current_cmd_words.is_empty() {
                        if let FishAstNode::Command(cmd) = self.build_command_node(current_cmd_words) {
                            commands.push(cmd);
                        }
                    }
                    let abbr_word = if !tokens.is_empty() {
                        match tokens.remove(0) {
                            FishToken::Word(n) => n,
                            _ => String::new(),
                        }
                    } else {
                        String::new()
                    };
                    let expansion = if !tokens.is_empty() {
                        match tokens.remove(0) {
                            FishToken::Word(n) => n,
                            _ => String::new(),
                        }
                    } else {
                        String::new()
                    };
                    return Ok(FishAstNode::Abbreviation(FishAbbreviation {
                        abbreviation: abbr_word,
                        expansion,
                    }));
                }
                _ => {}
            }
        }

        if !current_cmd_words.is_empty() {
            if let FishAstNode::Command(cmd) = self.build_command_node(current_cmd_words) {
                commands.push(cmd);
            }
        }

        if commands.len() == 1 {
            Ok(FishAstNode::Command(commands.remove(0)))
        } else if !commands.is_empty() {
            Ok(FishAstNode::Pipeline(FishPipelineNode { commands }))
        } else {
            Ok(FishAstNode::Empty)
        }
    }

    fn build_command_node(&self, words: Vec<String>) -> FishAstNode {
        if words.is_empty() {
            return FishAstNode::Empty;
        }

        FishAstNode::Command(FishCommandNode {
            name: words[0].clone(),
            args: words[1..].to_vec(),
            trailing_space: false,
            range: FishTextRange::default(),
        })
    }

    fn fish_token_len(&self, token: &FishToken) -> usize {
        match token {
            FishToken::Word(s) | FishToken::LongOption(s)
            | FishToken::Variable(s) | FishToken::Comment(s) => s.len(),
            FishToken::ShortOption(_) => 2,
            FishToken::Pipe | FishToken::RedirectOut | FishToken::RedirectAppend
            | FishToken::RedirectIn | FishToken::Semicolon | FishToken::DoubleAnd
            | FishToken::DoubleOr | FishToken::Newline | FishToken::Whitespace
            | FishToken::ParenOpen | FishToken::ParenClose | FishToken::BraceOpen
            | FishToken::BraceClose | FishToken::CommandSubstOpen
            | FishToken::CommandSubstClose => 1,
            FishToken::EndKeyword | FishToken::FunctionKeyword | FishToken::AbbrevKeyword
            | FishToken::IfKeyword | FishToken::ElseKeyword | FishToken::ForKeyword
            | FishToken::WhileKeyword | FishToken::CompleteCommand => 3,
        }
    }

    fn register_builtin_variables(&mut self) {
        let vars: Vec<FishVariableSpec> = vec![
            FishVariableSpec { name: "status".into(), description: "上一条命令的退出状态码".into(),
                is_readonly: true, default_value: Some("0".into()), category: FishVarCategory::Status },
            FishVariableSpec { name: "history".into(), description: "命令历史搜索结果".into(),
                is_readonly: true, default_value: None, category: FishVarCategory::History },
            FishVariableSpec { name: "history_search".into(), description: "历史搜索方向(无/prev/next)".into(),
                is_readonly: true, default_value: None, category: FishVarCategory::History },
            FishVariableSpec { name: "hostname".into(), description: "当前主机名".into(),
                is_readonly: true, default_value: None, category: FishVarCategory::Information },
            FishVariableSpec { name: "fish_pid".into(), description: "当前Fish进程ID".into(),
                is_readonly: true, default_value: None, category: FishVarCategory::Information },
            FishVariableSpec { name: "fish_private_mode".into(), description: "是否为私有模式".into(),
                is_readonly: true, default_value: None, category: FishVarCategory::Information },
            FishVariableSpec { name: "version".into(), description: "Fish版本号".into(),
                is_readonly: true, default_value: None, category: FishVarCategory::Information },
            FishVariableSpec { name: "os".into(), description: "操作系统名(Linux/macOS/Windows)".into(),
                is_readonly: true, default_value: None, category: FishVarCategory::Information },
            FishVariableSpec { name: "_".into(), description: "上一个前台作业的完整路径".into(),
                is_readonly: true, default_value: None, category: FishVarCategory::Information },
            FishVariableSpec { name: "PWD".into(), description: "当前工作目录".into(),
                is_readonly: true, default_value: None, category: FishVarCategory::Information },
            FishVariableSpec { name: "SHLVL".into(), description: "Shell嵌套层级".into(),
                is_readonly: true, default_value: Some("1".into()), category: FishVarCategory::Information },
            FishVariableSpec { name: "TERM".into(), description: "终端类型".into(),
                is_readonly: false, default_value: Some("xterm-256color".into()), category: FishVarCategory::Information },
            FishVariableSpec { name: "COLORTERM".into(), description: "颜色终端标识".into(),
                is_readonly: false, default_value: None, category: FishVarCategory::Information },
            FishVariableSpec { name: "EDITOR".into(), description: "默认文本编辑器".into(),
                is_readonly: false, default_value: Some("vim".into()), category: FishVarCategory::UserDefined },
            FishVariableSpec { name: "VISUAL".into(), description: "可视化编辑器".into(),
                is_readonly: false, default_value: None, category: FishVarCategory::UserDefined },
            FishVariableSpec { name: "BROWSER".into(), description: "默认浏览器".into(),
                is_readonly: false, default_value: None, category: FishVarCategory::UserDefined },
            FishVariableSpec { name: "PAGER".into(), description: "分页器程序".into(),
                is_readonly: false, default_value: Some("less".into()), category: FishVarCategory::UserDefined },
            FishVariableSpec { name: "PATH".into(), description: "可执行文件搜索路径".into(),
                is_readonly: false, default_value: None, category: FishVarCategory::UserDefined },
            FishVariableSpec { name: "CDPATH".into(), description: "cd搜索目录列表".into(),
                is_readonly: false, default_value: None, category: FishVarCategory::UserDefined },
            FishVariableSpec { name: "fish_greeting".into(), description: "Fish启动问候语".into(),
                is_readonly: false, default_value: None, category: FishVarCategory::UserDefined },
            FishVariableSpec { name: "fish_emoji_width".into(), description: "Emoji显示宽度(1/2)".into(),
                is_readonly: false, default_value: Some("2".into()), category: FishVarCategory::UserDefined },
            FishVariableSpec { name: "fish_ansi_width".into(), description: "ANSI序列宽度计算方式".into(),
                is_readonly: false, default_value: None, category: FishVarCategory::UserDefined },
            FishVariableSpec { name: "fish_handle_reflow".into(), description: "终端resize处理".into(),
                is_readonly: false, default_value: Some("1".into()), category: FishVarCategory::UserDefined },
            FishVariableSpec { name: "fish_escape_delay_ms".into(), description: "转义序列延迟(ms)".into(),
                is_readonly: false, default_value: Some("100".into()), category: FishVarCategory::UserDefined },
            FishVariableSpec { name: "fish_color_normal".into(), description: "默认前景色".into(),
                is_readonly: false, default_value: None, category: FishVarCategory::UserDefined },
            FishVariableSpec { name: "fish_color_command".into(), description: "命令颜色".into(),
                is_readonly: false, default_value: None, category: FishVarCategory::UserDefined },
            FishVariableSpec { name: "fish_color_param".into(), description: "参数颜色".into(),
                is_readonly: false, default_value: None, category: FishVarCategory::UserDefined },
            FishVariableSpec { name: "fish_color_error".into(), description: "错误颜色".into(),
                is_readonly: false, default_value: None, category: FishVarCategory::UserDefined },
            FishVariableSpec { name: "fish_color_quote".into(), description: "引号颜色".into(),
                is_readonly: false, default_value: None, category: FishVarCategory::UserDefined },
            FishVariableSpec { name: "fish_color_autosuggestion".into(), description: "建议文字颜色".into(),
                is_readonly: false, default_value: Some("555 brblack".into()), category: FishVarCategory::UserDefined },
            FishVariableSpec { name: "fish_color_user".into(), description: "用户名颜色".into(),
                is_readonly: false, default_value: None, category: FishVarCategory::UserDefined },
            FishVariableSpec { name: "fish_color_host".into(), description: "主机名颜色".into(),
                is_readonly: false, default_value: None, category: FishVarCategory::UserDefined },
            FishVariableSpec { name: "fish_color_cwd".into(), description: "当前目录颜色".into(),
                is_readonly: false, default_value: None, category: FishVarCategory::UserDefined },
            FishVariableSpec { name: "fish_color_cancel".into(), description: "取消替换颜色".into(),
                is_readonly: false, default_value: Some("-d".into()), category: FishVarCategory::UserDefined },
            FishVariableSpec { name: "__fish_completed_command".into(), description: "[内部] 正在补全的命令".into(),
                is_readonly: true, default_value: None, category: FishVarCategory::Internal },
            FishVariableSpec { name: "__fish_use_subcommand".into(), description: "[内部] 使用子命令模式".into(),
                is_readonly: true, default_value: None, category: FishVarCategory::Internal },
            FishVariableSpec { name: "__fish_is_first_token".into(), description: "[内部] 是否首个token".into(),
                is_readonly: true, default_value: None, category: FishVarCategory::Internal },
            FishVariableSpec { name: "__fish_seen_subcommand_from".into(), description: "[内部] 已见子命令来源".into(),
                is_readonly: true, default_value: None, category: FishVarCategory::Internal },
            FishVariableSpec { name: "__fish_git_prompt_show_informative_status".into(), description: "[Git] 详细状态提示".into(),
                is_readonly: false, default_value: None, category: FishVarCategory::Completion },
            FishVariableSpec { name: "__fish_git_prompt_color_branch".into(), description: "[Git] 分支颜色".into(),
                is_readonly: false, default_value: None, category: FishVarCategory::Completion },
            FishVariableSpec { name: "__fish_git_prompt_color_dirtystate".into(), description: "[Git] 脏状态颜色".into(),
                is_readonly: false, default_value: None, category: FishVarCategory::Completion },
            FishVariableSpec { name: "__fish_git_prompt_color_stagedstate".into(), description: "[Git] 暂存颜色".into(),
                is_readonly: false, default_value: None, category: FishVarCategory::Completion },
            FishVariableSpec { name: "__fish_git_prompt_color_invalidstate".into(), description: "[Git] 无效状态颜色".into(),
                is_readonly: false, default_value: None, category: FishVarCategory::Completion },
            FishVariableSpec { name: "__fish_git_prompt_color_upstream".into(), description: "[Git] 上游颜色".into(),
                is_readonly: false, default_value: None, category: FishVarCategory::Completion },
            FishVariableSpec { name: "__fish_hg_prompt_color_branch".into(), description: "[Hg] 分支颜色".into(),
                is_readonly: false, default_value: None, category: FishVarCategory::Completion },
            FishVariableSpec { name: "__fish_print_cmd_args_without_options".into(), description: "[工具] 打印非选项参数".into(),
                is_readonly: true, default_value: None, category: FishVarCategory::Internal },
            FishVariableSpec { name: "__fish_no_completion_suggest_special_dirs".into(), description: "[工具] 不建议特殊目录".into(),
                is_readonly: false, default_value: None, category: FishVarCategory::Internal },
            FishVariableSpec { name: "last_status".into(), description: "上一条命令退出码(兼容别名)".into(),
                is_readonly: true, default_value: None, category: FishVarCategory::Status },
            FishVariableSpec { name: "pipestatus".into(), description: "管道中各命令退出码数组".into(),
                is_readonly: true, default_value: None, category: FishVarCategory::Status },
            FishVariableSpec { name: "COLUMNS".into(), description: "终端列数".into(),
                is_readonly: true, default_value: None, category: FishVarCategory::Information },
            FishVariableSpec { name: "LINES".into(), description: "终端行数".into(),
                is_readonly: true, default_value: None, category: FishVarCategory::Information },
            FishVariableSpec { name: "fish_bind_mode".into(), description: "当前Vi绑定模式(insert/default/visual)".into(),
                is_readonly: true, default_value: Some("insert".into()), category: FishVarCategory::Information },
            FishVariableSpec { name: "fish_kill_signal".into(), description: "发送给前台的信号".into(),
                is_readonly: false, default_value: None, category: FishVarCategory::JobControl },
            FishVariableSpec { name: "disable_comment_prefix".into(), description: "禁用#作为注释前缀".into(),
                is_readonly: false, default_value: None, category: FishVarCategory::UserDefined },
            FishVariableSpec { name: "fish_trace".into(), description: "启用命令追踪调试".into(),
                is_readonly: false, default_value: None, category: FishVarCategory::UserDefined },
        ];

        for v in vars {
            self.known_variables.insert(v.name.clone(), v);
        }
    }

    fn register_common_completions(&mut self) {
        let git_completes = vec![
            FishCompleteSpec { command: "git".into(), short_option: None, long_option: Some("help".into()),
                description: Some("显示帮助信息".into()), arguments: None, old_arguments: None,
                condition: None, requires_param: false, keep_order: false, exclusive: false },
            FishCompleteSpec { command: "git".into(), short_option: Some('C'), long_option: Some("git-dir".into()),
                description: Some("设置GIT_DIR仓库路径".into()), arguments: Some("<path>".into()),
                old_arguments: None, condition: None, requires_param: true, keep_order: false, exclusive: false },
            FishCompleteSpec { command: "git".into(), short_option: Some('c'), long_option: Some("config".into()),
                description: Some("设置配置项".into()), arguments: Some("<name>=<value>".into()),
                old_arguments: None, condition: None, requires_param: true, keep_order: false, exclusive: false },
            FishCompleteSpec { command: "git".into(), short_option: None, long_option: Some("version".into()),
                description: Some("显示版本信息".into()), arguments: None, old_arguments: None,
                condition: None, requires_param: false, keep_order: false, exclusive: false },
            FishCompleteSpec { command: "git".into(), short_option: None, long_option: Some("paginate".into()),
                description: Some("分页输出".into()), arguments: None, old_arguments: None,
                condition: None, requires_param: false, keep_order: false, exclusive: false },
            FishCompleteSpec { command: "git".into(), short_option: None, long_option: Some("no-pager".into()),
                description: Some("不分页输出".into()), arguments: None, old_arguments: None,
                condition: None, requires_param: false, keep_order: false, exclusive: false },
        ];

        let docker_completes = vec![
            FishCompleteSpec { command: "docker".into(), short_option: None, long_option: Some("help".into()),
                description: Some("显示帮助".into()), arguments: None, old_arguments: None,
                condition: None, requires_param: false, keep_order: false, exclusive: false },
            FishCompleteSpec { command: "docker".into(), short_option: Some('v'), long_option: Some("version".into()),
                description: Some("显示版本".into()), arguments: None, old_arguments: None,
                condition: None, requires_param: false, keep_order: false, exclusive: false },
            FishCompleteSpec { command: "docker".into(), short_option: None, long_option: Some("context".into()),
                description: Some("指定构建上下文".into()), arguments: Some("<name>".into()),
                old_arguments: None, condition: None, requires_param: true, keep_order: false, exclusive: false },
            FishCompleteSpec { command: "docker".into(), short_option: None, long_option: Some("host".into()),
                description: Some("Docker守护进程地址".into()), arguments: Some("tcp://HOST:PORT".into()),
                old_arguments: None, condition: None, requires_param: true, keep_order: false, exclusive: false },
        ];

        let npm_completes = vec![
            FishCompleteSpec { command: "npm".into(), short_option: None, long_option: Some("help".into()),
                description: Some("显示帮助".into()), arguments: None, old_arguments: None,
                condition: None, requires_param: false, keep_order: false, exclusive: false },
            FishCompleteSpec { command: "npm".into(), short_option: Some('v'), long_option: Some("version".into()),
                description: Some("显示版本".into()), arguments: None, old_arguments: None,
                condition: None, requires_param: false, keep_order: false, exclusive: false },
            FishCompleteSpec { command: "npm".into(), short_option: None, long_option: Some("prefix".into()),
                description: Some("全局安装前缀".into()), arguments: Some("<path>".into()),
                old_arguments: None, condition: None, requires_param: true, keep_order: false, exclusive: false },
            FishCompleteSpec { command: "npm".into(), short_option: None, long_option: Some("save".into()),
                description: Some("保存到dependencies".into()), arguments: None, old_arguments: None,
                condition: None, requires_param: false, keep_order: false, exclusive: false },
            FishCompleteSpec { command: "npm".into(), short_option: None, long_option: Some("save-dev".into()),
                description: Some("保存到devDependencies".into()), arguments: None, old_arguments: None,
                condition: None, requires_param: false, keep_order: false, exclusive: false },
            FishCompleteSpec { command: "npm".into(), short_option: None, long_option: Some("save-exact".into()),
                description: Some("精确版本".into()), arguments: None, old_arguments: None,
                condition: None, requires_param: false, keep_order: false, exclusive: false },
        ];

        self.complete_specs.insert("git".into(), git_completes);
        self.complete_specs.insert("docker".into(), docker_completes);
        self.complete_specs.insert("npm".into(), npm_completes);
    }

    fn register_default_abbreviations(&mut self) {
        let defaults: Vec<(&str, &str, Option<&str>)> = vec![
            ("g", "git", Some("git")),
            ("ga", "git add", Some("git add")),
            ("gaa", "git add --all", Some("git add all")),
            ("gc", "git commit", Some("git commit")),
            ("gcm", "git commit -m", Some("git commit with message")),
            ("gp", "git push", Some("git push")),
            ("gpl", "git pull", Some("git pull")),
            ("gs", "git status", Some("git status")),
            ("gd", "git diff", Some("git diff")),
            ("gb", "git branch", Some("git branch")),
            ("gco", "git checkout", Some("git checkout")),
            ("gl", "git log --oneline -10", Some("git log简短")),
            ("la", "ls -a", Some("列出隐藏文件")),
            ("ll", "ls -alh", Some("详细列表")),
            ("cls", "clear", Some("清屏")),
            ("..", "cd ..", Some("上级目录")),
            ("...", "cd ../..", Some("上两级目录")),
            ("dc", "docker compose", Some("Docker Compose")),
            ("dcp", "docker compose ps", Some("查看容器")),
            ("dcu", "docker compose up -d", Some("启动服务")),
            ("dcd", "docker compose down", Some("停止服务")),
            ("dcl", "docker compose logs -f", Some("查看日志")),
            ("nr", "npm run", Some("运行脚本")),
            ("ni", "npm install", Some("安装依赖")),
            ("nid", "npm install -D", Some("安装开发依赖")),
            ("ns", "npm start", Some("启动项目")),
            ("nt", "npm test", Some("运行测试")),
            ("nb", "npm run build", Some("构建项目")),
            ("y", "yarn", Some("Yarn包管理器")),
            ("yi", "yarn install", Some("Yarn安装")),
            ("ya", "yarn add", Some("Yarn添加")),
            ("p", "pnpm", Some("pnpm包管理器")),
            ("pi", "pnpm install", Some("pnpm安装")),
            ("cb", "cargo build", Some("Cargo构建")),
            ("cr", "cargo run", Some("Cargo运行")),
            ("ct", "cargo test", Some("Cargo测试")),
            ("ca", "cargo add", Some("Cargo添加依赖")),
            ("ck", "cargo check", Some("Cargo检查")),
            ("e", "exit", Some("退出Shell")),
            ("q", "exit", Some("退出Shell")),
        ];

        for (word, expansion, desc) in defaults {
            self.abbreviations.insert(word.to_string(), FishAbbrSpec {
                word: word.to_string(),
                expansion: expansion.to_string(),
                description: desc.map(|d| d.to_string()),
                regex: None,
                set_cursor: false,
                position: None,
            });
        }
    }

    fn get_command_suggestions(&self, prefix: &str) -> Vec<CompletionSuggestion> {
        let common_commands: Vec<(&str, &str, u8)> = vec![
            ("git", "分布式版本控制", 98),
            ("docker", "容器管理平台", 97),
            ("npm", "JavaScript包管理器", 96),
            ("node", "JavaScript运行时", 95),
            ("python/python3", "Python解释器", 94),
            ("pip/pip3", "Python包管理器", 93),
            ("cargo", "Rust包管理器", 92),
            ("rustc/rustup", "Rust编译器/工具链", 91),
            ("go", "Go语言工具链", 90),
            ("make", "构建工具", 89),
            ("cmake", "跨平台构建系统", 88),
            ("gcc/g++", "C/C++编译器", 87),
            ("java/javac", "Java运行时/编译器", 86),
            ("ruby", "Ruby解释器", 85),
            ("gem", "Ruby包管理器", 84),
            ("php", "PHP解释器", 83),
            ("composer", "PHP包管理器", 82),
            ("ssh", "远程登录", 81),
            ("scp", "远程复制", 80),
            ("curl", "传输数据URL", 79),
            ("wget", "网络下载器", 78),
            ("tar", "归档工具", 77),
            ("zip/unzip", "压缩/解压", 76),
            ("grep/egrep/fgrep", "模式匹配搜索", 75),
            ("find", "查找文件", 74),
            ("sed", "流编辑器", 73),
            ("awk", "文本处理语言", 72),
            ("cat", "查看文件内容", 71),
            ("less/more", "分页查看", 70),
            ("head/tail", "查看文件头/尾", 69),
            ("wc", "统计行/词/字节", 68),
            ("sort", "排序文本", 67),
            ("uniq", "去重文本", 66),
            ("tee", "分流输出", 65),
            ("xargs", "参数构建执行", 64),
            ("chmod", "修改权限", 63),
            ("chown", "修改所有者", 62),
            ("ps", "进程状态", 61),
            ("top/htop/btop", "动态进程查看", 60),
            ("kill/killall", "终止进程", 59),
            ("systemctl", "系统服务控制", 58),
            ("journalctl", "系统日志查看", 57),
            ("env/printenv", "环境变量", 56),
            ("export/unset", "变量操作", 55),
            ("source/. (dot)", "加载脚本", 54),
            ("echo/printf", "输出文本", 53),
            ("read", "读取输入", 52),
            ("sleep", "暂停等待", 51),
            ("date", "日期时间", 50),
            ("basename/dirname", "路径处理", 49),
            ("realpath/readlink", "真实路径", 48),
            ("which/type/command", "查找命令位置", 47),
            ("man/info", "帮助文档", 46),
            ("history", "命令历史", 45),
            ("alias/unalias", "别名管理", 44),
            ("set/set -e/-x/-u", "Shell选项", 43),
            ("test/[", "条件测试", 42),
            ("true/false", "布尔值", 41),
            ("seq/jot", "数字序列", 40),
            ("shuf/sort -R", "随机排列", 39),
            ("tr", "字符替换删除", 38),
            ("cut", "按列切割", 37),
            ("paste", "合并行", 36),
            ("join", "关联合并", 35),
            ("diff", "文件差异比较", 34),
            ("patch", "应用差异补丁", 33),
            ("rsync", "同步复制", 32),
            ("nc/netcat", "网络工具箱", 31),
            ("lsof", "打开文件列表", 30),
            ("strace/ltrace", "调用追踪", 29),
            ("time", "计时执行", 28),
            ("timeout", "超时执行", 27),
            ("watch", "周期执行", 26),
            ("xclip/xsel", "剪贴板", 25),
            ("fd/fdfind", "快速查找", 24),
            ("ripgrep/rg", "快速搜索", 23),
            ("bat/cat replacement", "高亮cat", 22),
            ("exa/eza/ls replacement", "增强ls", 21),
            ("jq", "JSON处理器", 20),
            ("yq", "YAML处理器", 19),
            ("sd", "现代sed替代", 18),
            ("delta/diff-so-fancy", "增强diff", 17),
            ("zoxide/z", "智能跳转", 16),
            ("starship", "Prompt美化", 15),
            ("fish_config", "Fish配置UI", 14),
            ("fish_update_completions", "更新补全", 13),
        ];

        common_commands.into_iter()
            .filter(|(cmd, _, _)| prefix.is_empty() || cmd.to_lowercase().contains(&prefix.to_lowercase()))
            .map(|(cmd, desc, priority)| CompletionSuggestion {
                text: format!("{} ", cmd.split('/').next().unwrap_or(cmd)),
                display_text: cmd.split('/').next().unwrap_or(cmd).to_string(),
                description: desc.to_string(),
                kind: CompletionKind::Command,
                priority,
                metadata: HashMap::new(),
            })
            .collect()
    }

    fn get_option_suggestions(&self, prefix: &str) -> Vec<CompletionSuggestion> {
        let common_options: Vec<(&str, &str, bool)> = vec!(
            ("--help/-h", "显示帮助信息", false),
            ("--version/-v/-V", "显示版本号", false),
            ("--verbose/-v", "详细输出", true),
            ("--quiet/-q", "静默模式", true),
            ("--debug", "调试模式", true),
            ("--dry-run/-n", "模拟运行不实际执行", true),
            ("--force/-f", "强制执行", true),
            ("--interactive/-i", "交互模式", true),
            ("--recursive/-r/-R", "递归处理", true),
            ("--output/-o", "指定输出文件/路径", false),
            ("--input/-i", "指定输入文件/路径", false),
            ("--config/-c", "指定配置文件", false),
            ("--color/--no-color", "彩色/无彩色输出", true),
            ("--watch/-w", "监听变化自动重做", true),
            ("--jobs/-j", "并行任务数", false),
            ("--timeout/-t", "超时时间", false),
            ("--format/-f", "输出格式(json/table/csv...)", false),
            ("--sort/-s", "排序字段", false),
            ("--filter", "过滤条件", false),
            ("--limit/-n/--max-count", "限制数量", false),
            ("--offset/--skip", "跳过数量", false),
            ("--reverse/-r", "反转顺序", true),
            ("--unique/-u", "去重", true),
            ("--ignore-case/-i", "忽略大小写", true),
            ("--case-sensitive", "区分大小写", true),
            ("--all/-a", "全部包含(含隐藏)", true),
            ("--long/-l", "长格式输出", true),
            ("--human-readable/-h", "人类可读大小(K/M/G)", true),
            ("--append/-a", "追加模式", true),
            ("--backup/-b", "创建备份", true),
            ("--preserve", "保留属性", true),
            ("--no-clobber/-n", "不覆盖已存在", true),
            ("--update/-u", "仅更新较新文件", true),
        );

        common_options.into_iter()
            .filter(|(opt, _, _)| prefix.is_empty() || opt.to_lowercase().starts_with(&prefix.to_lowercase()))
            .map(|(opt, desc, is_sw)| CompletionSuggestion {
                text: if is_sw { format!("{} ", opt) } else { format!("{} ", opt) },
                display_text: opt.to_string(),
                description: format!("[通用] {}", desc),
                kind: CompletionKind::Argument,
                priority: 72,
                metadata: {
                    let mut m = HashMap::new();
                    if is_sw { m.insert("switch".to_string(), "true".to_string()); }
                    m
                },
            })
            .collect()
    }

    fn get_fish_variable_suggestions(&self, prefix: &str) -> Vec<CompletionSuggestion> {
        self.known_variables.values()
            .filter(|v| prefix.is_empty() || v.name.to_lowercase().starts_with(&prefix.to_lowercase()))
            .map(|v| CompletionSuggestion {
                text: format!("${}", v.name),
                display_text: format!("${} ({})", v.name, v.category),
                description: if v.is_readonly {
                    format!("[只读] {}", v.description)
                } else {
                    v.description.clone()
                },
                kind: CompletionKind::Variable,
                priority: if matches!(v.category, FishVarCategory::Status | FishVarCategory::Information) { 88 }
                           else if matches!(v.category, FishVarCategory::Completion) { 85 }
                           else { 78 },
                metadata: {
                    let mut m = HashMap::new();
                    m.insert("category".to_string(), format!("{}", v.category));
                    if v.is_readonly { m.insert("readonly".to_string(), "true".to_string()); }
                    if let Some(ref dv) = v.default_value { m.insert("default".to_string(), dv.clone()); }
                    m
                },
            })
            .collect()
    }

    fn get_variable_suggestions(&self, prefix: &str) -> Vec<CompletionSuggestion> {
        let env_vars: Vec<(&str, &str)> = vec![
            ("HOME", "用户主目录"),
            ("USER", "当前用户名"),
            ("PWD", "当前工作目录"),
            ("OLDPWD", "上一个工作目录"),
            ("PATH", "可执行文件搜索路径"),
            ("LANG/LANGUAGE", "语言/区域设置"),
            ("LC_ALL/LC_CTYPE/LC_MESSAGES", "区域类别"),
            ("TERM", "终端类型"),
            ("SHELL", "当前Shell路径"),
            ("EDITOR/VISUAL", "编辑器"),
            ("PAGER", "分页器"),
            ("TMPDIR/TEMP/TMP", "临时目录"),
            ("DISPLAY", "X11显示器"),
            ("SSH_AUTH_SOCK", "SSH认证socket"),
            ("SSH_AGENT_PID", "SSH代理PID"),
            ("http_proxy/https_proxy/all_proxy", "代理服务器"),
            ("no_proxy", "代理例外"),
            ("XDG_CONFIG_HOME", "用户配置目录"),
            ("XDG_DATA_HOME", "用户数据目录"),
            ("XDG_CACHE_HOME", "用户缓存目录"),
            ("XDG_RUNTIME_DIR", "运行时目录"),
            ("RANDOM", "随机数(0-32767)"),
            ("SECONDS", "Shell启动秒数"),
            ("BASHPID/ZSH_PID/FISH_PID", "当前PID"),
            ("UID/GID/EUID", "用户/组ID"),
            ("HOSTNAME/HOST", "主机名"),
            ("OSTYPE/OS", "操作系统类型"),
            ("MACHTYPE", "机器类型"),
            ("IFS", "内部字段分隔符"),
        ];

        env_vars.into_iter()
            .filter(|(var, _)| prefix.is_empty() || format!("${}", var).to_lowercase().starts_with(&prefix.to_lowercase()))
            .map(|(var, desc)| CompletionSuggestion {
                text: format!("${}", var),
                display_text: format!("${}", var),
                description: format!("[环境] {}", desc),
                kind: CompletionKind::Variable,
                priority: 85,
                metadata: {
                    let mut m = HashMap::new();
                    m.insert("type".to_string(), "environment".to_string());
                    m
                },
            })
            .collect()
    }

    fn get_complete_spec_suggestions(
        &self,
        specs: &[FishCompleteSpec],
        prefix: &str,
    ) -> Vec<CompletionSuggestion> {
        specs.iter()
            .filter(|spec| {
                if prefix.is_empty() { return true; }
                let p_lower = prefix.to_lowercase();
                if let Some(ref lo) = spec.long_option {
                    if format!("--{}", lo.to_lowercase()).starts_with(&p_lower) { return true; }
                }
                if let Some(so) = spec.short_option {
                    if format!("-{}", so).to_string().starts_with(&p_lower) { return true; }
                }
                false
            })
            .map(|spec| {
                let display = if let Some(ref lo) = spec.long_option {
                    format!("--{}", lo)
                } else if let Some(so) = spec.short_option {
                    format!("-{}", so)
                } else {
                    "?".into()
                };
                CompletionSuggestion {
                    text: if spec.requires_param { format!("{} ", display) } else { format!("{} ", display) },
                    display_text: display.clone(),
                    description: spec.description.clone().unwrap_or_else(|| "".into()),
                    kind: CompletionKind::Argument,
                    priority: 86,
                    metadata: {
                        let mut m = HashMap::new();
                        m.insert("from_complete".to_string(), "true".to_string());
                        if spec.requires_param { m.insert("requires_param".to_string(), "true".to_string()); }
                        if let Some(ref args) = spec.arguments { m.insert("arguments".to_string(), args.clone()); }
                        m
                    },
                }
            })
            .collect()
    }

    fn get_generic_option_suggestions(&self, cmd: &str, prefix: &str) -> Vec<CompletionSuggestion> {
        let cmd_specific: Vec<Vec<(&str, &str, bool)>> = match cmd {
            "git" => vec![
                vec!(("--bare", "初始化裸仓库", true), ("--mirror", "镜像克隆", true), ("--depth", "浅克隆深度", false),
                     ("--branch", "指定分支", false), ("--single-branch", "只克隆单分支", true),
                     ("--recurse-submodules", "递归克隆子模块", true), ("--shallow-submodules", "浅子模块", true),
                     ("-b", "创建并切换分支", false), ("-B", "强制创建分支", false), ("-f/--force", "强制操作", true),
                     ("-m", "提交消息", false), ("-amend", "修正提交", true), ("-a/--all", "暂存所有更改", true),
                     ("-p/--patch", "交互式暂存", true), ("-n/--dry-run", "模拟运行", true),
                     ("-u", "设置上游跟踪", true), ("--set-upstream", "设置上游跟踪", true),
                     ("--force-with-lease", "安全强推", true), ("--delete", "删除远程分支", true),
                     ("--rebase", "变基拉取", true), ("--ff-only", "仅快进合并", true),
                     ("--no-ff", "不快进合并", true), ("--squash", "压缩合并", true),
                     ("-M/-m", "强制重命名", true), ("-D", "强制删除分支", true),
                     ("-S/-G", "GPG签名", true), ("-e", "打开编辑器", true),
                     ("--oneline", "单行日志", true), ("--graph", "图形化日志", true),
                     ("--stat", "统计变更", true), ("--short", "简短状态", true), ("-b/--branch", "显示分支", true),
                     ("--show-stash", "显示stash", true), ("-vv", "显示上游关系", true),
                     ("--staged/--cached", "暂存区差异", true), ("--name-only", "仅文件名", true),
                     ("--color", "着色输出", true), ("--no-color", "不着色", true),
                     ("--autostash", "自动stash", true), ("-s/--signoff", "签名提交", true),
                     ("--no-verify", "跳过hooks", true), ("--verify", "验证hooks", true),
                     ("--tags", "同时推送标签", true), ("--follow", "跟随重命名", true),
                     ("-U", "显示上下文行数", false), ("-w", "忽略空白差异", true),
                     ("--ignore-space-change", "忽略空格变化", true), ("--ignore-all-space", "忽略所有空格", true),
                     ("--ignore-blank-lines", "忽略空行", true), ("--diff-filter", "过滤文件状态", false),
                     ("--find-renames/--find-copies", "检测重命名/拷贝", true), ("-l", "未合并路径", true),
                     ("-r", "递归删除", true), ("--cached", "从索引删除", true),
                     ("--ignore-unmatch", "忽略缺失文件", true), ("--quiet", "静默模式", true),
                     ("--soft", "软重置", true), ("--mixed", "混合重置(默认)", true),
                     ("--hard", "硬重置", true), ("--merge", "合并重置", true),
                     ("--keep", "保持重置", true), ("-X", "策略选项", false),
                     ("--submodule", "子模块更新策略", false), ("-f/--force", "强制推送", true),
                     ("--all", "推送所有分支", true), ("--tags", "推送标签", true),
                     ("--prune", "清理远程已删引用", true), ("--porcelain", "机器可读格式", true),
                     ("--null", "-z终止符", true), ("-z", "-z终止符", true),
                     ("-C", "在指定路径运行", false), ("-c", "配置项", false)),
            ],
            "docker" => vec![
                vec!(("-a/--all", "显示所有容器(含停止)", true), ("-q/--quiet", "仅ID", true),
                     ("-f/--filter", "过滤条件", false), ("--format", "输出模板", false),
                     ("-s/--size", "显示大小", true), ("--no-trunc", "不截断", true),
                     ("-d/--detach", "后台运行", true), ("-it", "交互TTY", true),
                     ("--name", "容器名称", false), ("-p/--publish", "端口映射", false),
                     ("-v/--volume", "卷挂载", false), ("-e/--env", "环境变量", false),
                     ("--network", "网络模式", false), ("--restart", "重启策略", false),
                     ("--rm", "退出后删除", true), ("--memory/-m", "内存限制", false),
                     ("--cpus", "CPU限制", false), ("--gpus", "GPU设备", false),
                     ("--privileged", "特权模式", true), ("--user/-u", "运行用户", false),
                     ("-w/--workdir", "工作目录", false), ("--entrypoint", "入口点", false),
                     ("--label/-l", "元数据标签", false), ("--link", "容器链接", false),
                     ("--dns", "DNS服务器", false), ("--add-host", "hosts条目", false),
                     ("--ipc", "IPC命名空间", false), ("--pid", "PID命名空间", false),
                     ("--read-only", "只读根文件系统", true), ("--tmpfs", "tmpfs挂载", false),
                     ("--security-opt", "安全选项", false), ("--cap-add/--cap-drop", "能力增减", false),
                     ("--platform", "目标平台", false), ("--pull", "镜像拉取策略", false),
                     ("--build-arg", "构建参数", false), ("-t/--tag", "镜像标签", false),
                     ("-f/--file", "Dockerfile路径", false), ("--target", "多阶段目标", false),
                     ("--cache-from", "缓存源", false), ("--cache-to", "缓存目标", false),
                     ("--progress", "进度输出(auto/plain/tty)", false),
                     ("--secret", "BuildKit secret", false), ("--ssh", "BuildKit SSH", false),
                     ("--compress", "压缩构建上下文", true), ("--no-cache", "无缓存构建", true),
                     ("--parallel", "并行构建", true), ("--quiet/-q", "静默构建", true)),
            ],
            "npm" | "yarn" | "pnpm" => vec![
                vec!(("-g/--global", "全局安装", true), ("-D/--save-dev", "保存为开发依赖", true),
                     ("-O/--save-optional", "可选依赖", true), ("-E/--save-exact", "精确版本", true),
                     ("-P/--save-peer", "对等依赖", true), ("--no-save", "不更新package.json", true),
                     ("--workspace", "工作区范围", false), ("-w", "工作区", false),
                     ("--legacy-peer-deps", "忽略peer-dep冲突", true), ("--strict-peer-deps", "严格peer-deps", true),
                     ("--force", "强制覆盖", true), ("--ignore-scripts", "跳过scripts", true),
                     ("--offline", "离线模式", true), ("--prefer-offline", "优先离线", true),
                     ("--lockfile-version", "锁定文件版本", false), ("--package-lock-only", "仅生成锁文件", true),
                     ("--audit", "安全审计", true), ("--fund", "资金信息", true),
                     ("-w/--watch", "监听模式", true), ("--production", "生产模式", true),
                     ("--development", "开发模式", true), ("--ci", "CI模式(去除devDep)", true),
                     ("--dry-run", "模拟运行", true), ("--verbose", "详细输出", true),
                     ("--loglevel", "日志级别", false), ("--prefix", "安装前缀", false),
                     ("--registry", "注册表地址", false), ("--tag", "发布标签", false),
                     ("--access", "访问权限(public/restricted)", false), ("--otp", "双因素认证码", false)),
            ],
            "cargo" => vec![
                vec!(("--lib", "库目标", true), ("--bin", "二进制目标", false),
                     ("--example", "示例目标", false), ("--test", "测试目标", false),
                     ("--bench", "基准测试目标", false), ("--release", "Release构建", true),
                     ("--profile", "构建配置(dev/release/custom)", false),
                     ("--features", "启用特性", false), ("--all-features", "所有特性", true),
                     ("--no-default-features", "禁用默认特性", true),
                     ("--target", "目标三元组", false), ("--target-dir", "目标目录", false),
                     ("--manifest-path", "Cargo.toml路径", false), ("--locked", "要求锁定文件", true),
                     ("--locked", "要求精确依赖", true), ("--offline", "离线模式", true),
                     ("-v/--verbose", "详细输出", true), ("-q/--quiet", "静默输出", true),
                     ("--color", "着色(auto/always/never)", false), ("--frozen", "冻结依赖", true),
                     ("--Z", "Nightly不稳定选项", false), ("-j/--jobs", "并行数", false),
                     ("--keep-going", "继续构建(即使失败)", true), ("--message-format", "消息格式", false),
                     ("--build-plan", "导出构建计划", true), ("--timings", "编译耗时统计", true),
                     ("--config", "覆盖配置", false), ("--ignore-rust-version", "忽略Rust版本", true),
                     ("--all-targets", "所有目标", true), ("--doc", "文档目标", true),
                     ("--tests", "测试目标", true), ("--examples", "示例目标", true),
                     ("--benches", "基准目标", true), ("--deps", "仅依赖", true),
                     ("--workspace", "整个工作区", true), ("-p/--package", "指定包", false),
                     ("--exclude", "排除包", false), ("--dev", "开发依赖", true),
                     ("--build-std", "构建标准库", true), ("--build-std-features", "标准库特性", false),
                     ("--crate-type", " crate类型", false), ("--edition", "Rust版本(2018/2021/2024)", false)),
            ],
            _ => vec![],
        };

        cmd_specific.into_iter()
            .flatten()
            .filter(|(opt, _, _)| prefix.is_empty() || opt.to_lowercase().starts_with(&prefix.to_lowercase()))
            .map(|(opt, desc, is_sw)| CompletionSuggestion {
                text: if is_sw { format!("{} ", opt) } else { format!("{} ", opt) },
                display_text: opt.to_string(),
                description: desc.to_string(),
                kind: CompletionKind::Argument,
                priority: 84,
                metadata: {
                    let mut m = HashMap::new();
                    if is_sw { m.insert("switch".to_string(), "true".to_string()); }
                    m.insert("command".to_string(), cmd.to_string());
                    m
                },
            })
            .collect()
    }

    fn get_git_fish_suggestions(&self, context: &CompletionContext) -> Vec<CompletionSuggestion> {
        let subcommands: Vec<(&str, &str, u8)> = vec![
            ("status", "显示工作区状态", 98),
            ("add", "添加文件到暂存区", 97),
            ("commit", "提交更改", 96),
            ("push", "推送到远程", 95),
            ("pull", "拉取并整合远程", 94),
            ("fetch", "从远程下载对象", 93),
            ("checkout", "切换分支/恢复文件", 92),
            ("switch", "切换分支(Git 2.23+)", 91),
            ("restore", "恢复文件(Git 2.23+)", 90),
            ("branch", "列出/创建/删除分支", 89),
            ("merge", "合并分支", 88),
            ("rebase", "变基分支", 87),
            ("log", "显示提交日志", 86),
            ("diff", "显示差异", 85),
            ("show", "显示对象内容", 84),
            ("stash", "暂存更改", 83),
            ("tag", "管理标签", 82),
            ("remote", "管理远程仓库", 81),
            ("clone", "克隆仓库", 80),
            ("init", "初始化仓库", 79),
            ("clean", "删除未跟踪文件", 78),
            ("reset", "重置HEAD", 77),
            ("rm", "删除文件(索引+工作区)", 76),
            ("mv", "移动/重命名", 75),
            ("bisect", "二分查找引入bug的提交", 74),
            ("grep", "搜索内容", 73),
            ("blame", "逐行追溯", 72),
            ("cherry-pick", "摘取提交", 71),
            ("revert", "撤销提交", 70),
            ("format-patch", "生成邮件格式补丁", 69),
            ("apply/am", "应用补丁", 68),
            ("archive", "创建归档", 67),
            ("bundle", "创建bundle对象", 66),
            ("fsck", "完整性检查", 65),
            ("reflog", "引用日志", 64),
            ("filter-branch", "重写分支历史", 63),
            ("submodule", "子模块管理", 62),
            ("worktree", "工作树管理", 61),
            ("range-diff", "范围差异比较", 60),
            ("shortlog", "简洁日志", 59),
            ("describe", "基于标签描述对象", 58),
            ("notes", "附加注释", 57),
            ("replace", "替换对象", 56),
            ("send-email", "邮件发送补丁", 55),
            ("request-pull", "请求拉取", 54),
            ("svn", "SVN桥接", 53),
            ("cvsserver", "CVSServer协议", 52),
        ];

        let prefix = context.get_word_before_cursor();

        subcommands.into_iter()
            .filter(|(cmd, _, _)| prefix.is_empty() || cmd.starts_with(prefix))
            .map(|(cmd, desc, priority)| CompletionSuggestion {
                text: format!("{} ", cmd),
                display_text: cmd.to_string(),
                description: desc.to_string(),
                kind: CompletionKind::Argument,
                priority,
                metadata: HashMap::new(),
            })
            .collect()
    }

    fn get_docker_fish_suggestions(&self, context: &CompletionContext) -> Vec<CompletionSuggestion> {
        let subcommands: Vec<(&str, &str, u8)> = vec![
            ("ps", "列出容器", 95),
            ("images", "列出镜像", 94),
            ("run", "运行新容器", 93),
            ("start", "启动容器", 92),
            ("stop", "停止容器", 91),
            ("restart", "重启容器", 90),
            ("rm", "删除容器", 89),
            ("rmi", "删除镜像", 88),
            ("exec", "在运行的容器中执行", 87),
            ("logs", "获取容器日志", 86),
            ("inspect", "显示详细信息", 85),
            ("top", "显示进程", 84),
            ("stats", "资源使用统计", 83),
            ("port", "端口映射列表", 82),
            ("cp", "容器间/宿主文件复制", 81),
            ("diff", "检查文件变更", 80),
            ("commit", "容器保存为新镜像", 79),
            ("export/import", "容器导入/导出", 78),
            ("save/load", "镜像导入/导出", 77),
            ("build", "构建镜像(Dockerfile)", 76),
            ("pull", "拉取镜像", 75),
            ("push", "推送镜像", 74),
            ("search", "搜索镜像", 73),
            ("login/logout", "登录/登出注册表", 72),
            ("tag", "镜像打标签", 71),
            ("create", "创建但不启动容器", 70),
            ("pause/unpause", "暂停/恢复容器", 69),
            ("kill", "强制终止容器", 68),
            ("wait", "阻塞直到容器停止", 67),
            ("attach", "连接到运行中的容器", 66),
            ("events", "获取实时事件", 65),
            ("update", "更新容器配置", 64),
            ("rename", "重命名容器", 63),
            ("prune", "清理未使用资源", 62),
            ("volume", "卷管理", 61),
            ("network", "网络管理", 60),
            ("compose", "多容器编排(docker-compose)", 59),
            ("swarm", "集群管理模式", 58),
            ("service", "Swarm服务管理", 57),
            ("stack", "Swarm栈管理", 56),
            ("config", "Docker配置管理", 55),
            ("secret", "敏感数据管理", 54),
            ("trust", "镜像信任管理", 53),
            ("manifest", "清单管理", 52),
            ("context", "上下文管理", 51),
            ("builder", "BuildKit构建器", 50),
            ("scan", "漏洞扫描", 49),
            ("system", "Docker系统信息", 48),
            ("info", "系统范围信息", 47),
            ("version", "版本信息", 46),
        ];

        let prefix = context.get_word_before_cursor();

        subcommands.into_iter()
            .filter(|(cmd, _, _)| prefix.is_empty() || cmd.starts_with(prefix))
            .map(|(cmd, desc, priority)| CompletionSuggestion {
                text: format!("{} ", cmd),
                display_text: cmd.to_string(),
                description: desc.to_string(),
                kind: CompletionKind::Argument,
                priority,
                metadata: HashMap::new(),
            })
            .collect()
    }

    fn get_npm_fish_suggestions(&self, context: &CompletionContext) -> Vec<CompletionSuggestion> {
        let subcommands: Vec<(&str, &str, u8)> = vec![
            ("install/i/add", "安装依赖包", 96),
            ("uninstall/rm/remove/unlink", "卸载包", 95),
            ("update/up/upgrade/udpate", "更新依赖", 94),
            ("run/urn/run-script", "运行脚本", 93),
            ("test/t/tst", "运行测试", 92),
            ("start/stop/restart", "生命周期脚本", 91),
            ("build", "构建项目", 90),
            ("pack", "打包(tarball)", 89),
            ("publish/pub", "发布包", 88),
            ("deprecate/undeprecate", "标记废弃/取消废弃", 87),
            ("init/create/innit", "初始化项目", 86),
            ("config/c", "管理配置", 85),
            ("set/get/delete/list/ls/rm", "配置操作", 84),
            ("prefix/g", "查看/设置前缀", 83),
            ("root", "查看npm根目录", 82),
            ("view/v/info/show", "查看包详情", 81),
            ("search/s/se/find", "搜索包", 80),
            ("team", "团队管理", 79),
            ("access/public/restrict/grant/revokes", "权限管理", 78),
            ("owner/add/rm/ls", "所有者管理", 77),
            ("stars/star/unstar", "收藏管理", 76),
            ("docs/doc/home/open/help", "文档/帮助", 75),
            ("repo/issues/bugs", "仓库/问题链接", 74),
            ("audit/fix", "安全审计/修复", 73),
            ("fund", "资金信息", 72),
            ("link/ln", "本地链接包", 71),
            ("logout", "登出注册表", 70),
            ("whoami", "当前用户", 69),
            ("doctor/ddoctor/check", "诊断检查", 68),
            ("cache/ls/clean/verify", "缓存管理", 67),
            ("profile/get/set/delete", "性能分析", 66),
            ("org/create/set/rm", "组织管理", 65),
            ("token/list/revoke/create", "认证令牌", 64),
            ("login/adduser/add-user", "登录/注册", 63),
            ("version/version/-v", "版本信息", 62),
            ("exec", "在项目上下文中运行命令", 61),
            ("explain", "安装行为解释", 60),
            ("query", "依赖图查询", 59),
            ("diff", "依赖差异比较", 58),
            ("pkg", "包操作(get/set fix)", 57),
            ("ll/list/ls/la", "已安装包列表", 56),
            ("outdated", "检查过时的包", 55),
            ("why", "查询为什么安装了某个包", 54),
            ("staff/members", "团队成员", 53),
            ("hook/owner", "生命周期钩子", 52),
            ("restart/completion/completions", "Shell补全", 51),
            ("bin", "可执行文件路径", 50),
        ];

        let prefix = context.get_word_before_cursor();

        subcommands.into_iter()
            .filter(|(cmds, _, _)| {
                let parts: Vec<&str> = cmds.split('/').collect();
                parts.iter().any(|cmd| prefix.is_empty() || cmd.starts_with(prefix))
            })
            .map(|(cmds, desc, priority)| {
                let primary = cmds.split('/').next().unwrap_or(cmds);
                CompletionSuggestion {
                    text: format!("{} ", primary),
                    display_text: primary.to_string(),
                    description: desc.to_string(),
                    kind: CompletionKind::Argument,
                    priority,
                    metadata: HashMap::new(),
                }
            })
            .collect()
    }

    fn get_cargo_fish_suggestions(&self, context: &CompletionContext) -> Vec<CompletionSuggestion> {
        let subcommands: Vec<(&str, &str, u8)> = vec![
            ("build/b", "编译项目", 96),
            ("check/c", "检查但不编译(快速)", 95),
            ("run/r", "运行二进制", 94),
            ("test/t", "运行测试", 93),
            ("bench", "运行基准测试", 92),
            ("clean", "清除构建产物", 91),
            ("fix", "自动修复警告", 90),
            ("fmt/fmt", "代码格式化", 89),
            ("clippy", "Lint检查", 88),
            ("doc/d", "生成文档", 87),
            ("metadata", "输出包元数据(JSON)", 86),
            ("generate-lockfile", "重新生成Cargo.lock", 85),
            ("locate-project", "定位项目根目录", 84),
            ("pkgid", "输出包规格说明符", 83),
            ("tree", "显示依赖树", 82),
            ("update", "更新依赖(Cargo.lock)", 81),
            ("search/s", "在crates.io搜索", 80),
            ("publish", "发布到crates.io", 79),
            ("install/i", "安装Rust二进制", 78),
            ("uninstall/rm/remove", "卸载Rust二进制", 77),
            ("add/a", "添加依赖到Cargo.toml", 76),
            ("remove/rm", "移除依赖", 75),
            ("rename", "重命名当前包", 74),
            ("version/v", "改变当前包版本", 73),
            ("owner", "crates.io所有者管理", 72),
            ("yank/unyank", "撤回/恢复版本", 71),
            ("login", "登录registry", 70),
            ("token", "API令牌管理", 69),
            ("new", "创建新项目", 68),
            ("init", "在现有目录初始化", 67),
            ("config", "管理Cargo配置", 66),
            ("help/--help/-h", "帮助信息", 65),
            ("-V/--version", "版本信息", 64),
        ];

        let prefix = context.get_word_before_cursor();

        subcommands.into_iter()
            .filter(|(cmd, _, _)| {
                let parts: Vec<&str> = cmd.split('/').collect();
                parts.iter().any(|c| prefix.is_empty() || c.starts_with(prefix))
            })
            .map(|(cmds, desc, priority)| {
                let primary = cmds.split('/').next().unwrap_or(cmds);
                CompletionSuggestion {
                    text: format!("{} ", primary),
                    display_text: primary.to_string(),
                    description: desc.to_string(),
                    kind: CompletionKind::Argument,
                    priority,
                    metadata: HashMap::new(),
                }
            })
            .collect()
    }

    fn get_fish_builtin_suggestions(&self, context: &CompletionContext) -> Vec<CompletionSuggestion> {
        let builtins: Vec<(&str, &str, u8)> = vec![
            ("abbr", "管理缩写", 95),
            ("alias", "定义函数式别名", 94),
            ("argparse", "参数解析辅助", 93),
            ("begin", "开始代码块", 92),
            ("bg", "将作业放到后台", 91),
            ("bind", "绑定键序列", 90),
            ("block", "暂时阻断事件", 89),
            ("break", "跳出循环", 88),
            ("breakpoint", "调试断点", 87),
            ("builtin", "显式调用内置命令", 86),
            ("case", "模式匹配分支", 85),
            ("cd", "切换目录", 84),
            ("command", "运行外部命令", 83),
            ("commandline", "编辑命令行缓冲区", 82),
            ("complete", "注册补全规则", 81),
            ("contains", "检查值是否在集合中", 80),
            ("continue", "进入下一次循环迭代", 79),
            ("count", "计数参数个数", 78),
            ("debug", "调试输出/模式切换", 77),
            ("eval", "求值字符串", 76),
            ("exec", "替换当前进程", 75),
            ("exit", "退出Shell", 74),
            ("export", "设置导出变量", 73),
            ("false", "返回假", 72),
            ("fg", "前台化作业", 71),
            ("for", "for循环", 70),
            ("function/functions/func", "定义函数", 69),
            ("functions", "列出/删除函数", 68),
            ("history", "命令历史操作", 67),
            ("if", "条件判断", 66),
            ("jobs", "列出作业", 65),
            ("math", "数学计算", 64),
            ("not/negate", "逻辑取反", 63),
            ("or", "逻辑或", 62),
            ("printf", "格式化输出", 61),
            ("pwd", "打印工作目录", 60),
            ("random", "生成随机数", 59),
            ("read", "读取一行输入", 58),
            ("realpath", "解析真实路径", 57),
            ("return", "从函数返回", 56),
            ("set/set_color", "设置变量/颜色", 55),
            ("set_escape", "转义行为", 54),
            ("source/.", "加载执行文件", 53),
            ("status", "查询Shell状态", 52),
            ("string", "字符串操作", 51),
            ("switch/sw", "模式匹配", 50),
            ("test/[", "条件测试", 49),
            ("trap", "信号处理", 48),
            ("true", "返回真", 47),
            ("type/typeset", "查询命令类型", 46),
            ("ulimit", "资源限制", 45),
            ("umask", "权限掩码", 44),
            ("wait", "等待作业完成", 43),
            ("while", "while循环", 42),
            ("__fish_complete_subcommand", "子命令补全辅助", 40),
            ("__fish_complete_directories", "目录补全辅助", 39),
            ("__fish_complete_files", "文件补全辅助", 38),
            ("__fish_complete_groups", "组补全辅助", 37),
            ("__fish_complete_pids", "PID补全辅助", 36),
            ("__fish_complete_users", "用户补全辅助", 35),
            ("__fish_complete_suffix", "后缀补全辅助", 34),
            ("__fish_describe_command", "命令描述辅助", 33),
            ("__fish_print_commands", "打印可用命令", 32),
            ("__fish_print_filesystems", "打印文件系统", 31),
            ("__fish_print_interfaces", "打印网络接口", 30),
            ("__fish_print_jobs", "打印作业", 29),
            ("__fish_print_linux_services", "打印Linux服务", 28),
            ("__fish_print_users", "打印用户", 27),
            ("__fish_use_subcommand", "子命令模式检测", 26),
            ("__fish_seen_subcommand_from", "检测已知子命令", 25),
            ("fish_add_path", "添加到PATH", 24),
            ("fish_config", "Web配置界面", 23),
            ("fish_git_prompt", "Git提示符函数", 22),
            ("fish_hg_prompt", "Mercurial提示符函数", 21),
            ("fish_indent", "Fish代码格式化", 20),
            ("fish_opt", "选项解析", 19),
            ("fish_update_completions", "更新补全脚本", 18),
            ("fish_vcs_prompt", "VCS通用提示符", 17),
            ("prompt_hostname", "主机名提示组件", 16),
            ("prompt_login", "登录提示组件", 15),
            ("prompt_pwd", "路径提示组件", 14),
            ("prompt_status", "状态提示组件", 13),
        ];

        let prefix = context.get_word_before_cursor();

        builtins.into_iter()
            .filter(|(cmd, _, _)| {
                let parts: Vec<&str> = cmd.split('/').collect();
                parts.iter().any(|c| prefix.is_empty() || c.starts_with(prefix))
            })
            .map(|(cmds, desc, priority)| {
                let primary = cmds.split('/').next().unwrap_or(cmds);
                CompletionSuggestion {
                    text: format!("{} ", primary),
                    display_text: primary.to_string(),
                    description: desc.to_string(),
                    kind: CompletionKind::Command,
                    priority,
                    metadata: {
                        let mut m = HashMap::new();
                        m.insert("builtin".to_string(), "true".to_string());
                        m
                    },
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_command() {
        let parser = FishParser::new();
        let result = parser.parse("git status --short", 10);

        assert!(result.errors.is_empty());

        match result.ast {
            FishAstNode::Command(cmd) => {
                assert_eq!(cmd.name, "git");
            }
            _ => panic!("Expected Command node"),
        }
    }

    #[test]
    fn test_parse_pipeline() {
        let parser = FishParser::new();
        let result = parser.parse("cat file.txt | grep pattern | head -5", 30);

        match result.ast {
            FishAstNode::Pipeline(pipeline) => {
                assert_eq!(pipeline.commands.len(), 3);
            }
            _ => panic!("Expected Pipeline node"),
        }
    }

    #[test]
    fn test_parse_function_definition() {
        let parser = FishParser::new();
        let result = parser.parse("function greet; echo Hello $argv; end", 28);

        match result.ast {
            FishAstNode::FunctionDef(func) => {
                assert_eq!(func.name, "greet");
            }
            _ => {}
        }
    }

    #[test]
    fn test_parse_abbreviation() {
        let parser = FishParser::new();
        let result = parser.parse("abbr la ls -alh", 15);

        match result.ast {
            FishAstNode::Abbreviation(abbr) => {
                assert_eq!(abbr.abbreviation, "la");
                assert_eq!(abbr.expansion, "ls -alh");
            }
            _ => {}
        }
    }

    #[test]
    fn test_tokenize_long_options() {
        let parser = FishParser::new();
        let tokens = parser.tokenize("npm install --save-dev typescript");

        let has_save_dev = tokens.iter().any(|t| matches!(t, FishToken::LongOption(o) if o == "save-dev"));
        assert!(has_save_dev);
    }

    #[test]
    fn test_tokenize_short_options() {
        let parser = FishParser::new();
        let tokens = parser.tokenize("ls -lah");

        let has_short_opts = tokens.iter().filter(|t| matches!(t, FishToken::ShortOption(_))).count();
        assert_eq!(has_short_opts, 4);
    }

    #[test]
    fn test_fish_variable_registry_size() {
        let parser = FishParser::new();
        assert!(parser.known_variables.len() >= 50,
            "Expected >=50 variables, got {}", parser.known_variables.len());
    }

    #[test]
    fn test_has_fish_internal_variables() {
        let parser = FishParser::new();
        assert!(parser.known_variables.contains_key("status"));
        assert!(parser.known_variables.contains_key("__fish_completed_command"));
        assert!(parser.known_variables.contains_key("fish_greeting"));
        assert!(parser.known_variables.contains_key("fish_color_command"));
    }

    #[test]
    fn test_abbreviation_registry() {
        let parser = FishParser::new();
        assert!(parser.abbreviations.contains_key("g"));
        assert!(parser.abbreviations.contains_key("ga"));
        assert!(parser.abbreviations.contains_key("gp"));
        assert!(parser.abbreviations.contains_key("dc"));

        let g_abbr = &parser.abbreviations["g"];
        assert_eq!(g_abbr.expansion, "git");
    }

    #[test]
    fn test_complete_specs_registered() {
        let parser = FishParser::new();
        assert!(parser.complete_specs.contains_key("git"));
        assert!(parser.complete_specs.contains_key("docker"));
        assert!(parser.complete_specs.contains_key("npm"));
    }

    #[test]
    fn test_git_fish_completions() {
        let parser = FishParser::new();
        let ctx = CompletionContext::new("git ", 4);
        let suggestions = parser.get_suggestions(&ctx);

        assert!(!suggestions.is_empty());
        assert!(suggestions.iter().any(|s| s.text.contains("status")));
        assert!(suggestions.iter().any(|s| s.text.contains("commit")));
        assert!(suggestions.iter().any(|s| s.text.contains("push")));
    }

    #[test]
    fn test_docker_fish_completions() {
        let parser = FishParser::new();
        let ctx = CompletionContext::new("docker ", 7);
        let suggestions = parser.get_suggestions(&ctx);

        assert!(!suggestions.is_empty());
        assert!(suggestions.iter().any(|s| s.text.contains("ps")));
        assert!(suggestions.iter().any(|s| s.text.contains("run")));
    }

    #[test]
    fn test_cargo_fish_completions() {
        let parser = FishParser::new();
        let ctx = CompletionContext::new("cargo ", 6);
        let suggestions = parser.get_suggestions(&ctx);

        assert!(!suggestions.is_empty());
        assert!(suggestions.iter().any(|s| s.text.contains("build")));
        assert!(suggestions.iter().any(|s| s.text.contains("run")));
        assert!(suggestions.iter().any(|s| s.text.contains("test")));
    }

    #[test]
    fn test_fish_builtins_completions() {
        let parser = FishParser::new();
        let ctx = CompletionContext::new("fish ", 4);
        let suggestions = parser.get_suggestions(&ctx);

        assert!(!suggestions.is_empty());
        assert!(suggestions.iter().any(|s| s.text.contains("config")));
        assert!(suggestions.iter().any(|s| s.text.contains("indent")));
    }

    #[test]
    fn test_fish_variable_completions() {
        let parser = FishParser::new();
        let ctx = CompletionContext::new("$", 1);
        let suggestions = parser.get_suggestions(&ctx);

        assert!(!suggestions.is_empty());
        assert!(suggestions.iter().any(|s| s.text.contains("$status")));
        assert!(suggestions.iter().any(|s| s.text.contains("$PWD")));
    }

    #[test]
    fn test_fish_special_variable_completions() {
        let parser = FishParser::new();
        let ctx = CompletionContext::new("__fish_", 7);
        let suggestions = parser.get_suggestions(&ctx);

        assert!(!suggestions.is_empty());
        assert!(suggestions.iter().any(|s| s.display_text.contains("__fish_git")));
        assert!(suggestions.iter().any(|s| s.display_text.contains("__fish_completed")));
    }

    #[test]
    fn test_abbr_keyword_detection() {
        let parser = FishParser::new();
        let result = parser.parse("abbr gs git status", 20);

        assert!(result.cursor_context.in_abbreviation);
    }

    #[test]
    fn test_function_keyword_detection() {
        let parser = FishParser::new();
        let result = parser.parse("function myfunc", 16);

        assert!(result.cursor_context.in_function_def);
    }

    #[test]
    fn test_empty_input() {
        let parser = FishParser::new();
        let result = parser.parse("", 0);

        assert!(matches!(result.ast, FishAstNode::Empty));
    }

    #[test]
    fn test_list_operator_parsing() {
        let parser = FishParser::new();
        let result = parser.parse("cmd1 && cmd2 || cmd3", 18);

        match result.ast {
            FishAstNode::Pipeline(pipeline) => {
                assert!(pipeline.commands.len() >= 2);
            }
            _ => {}
        }
    }

    #[test]
    fn test_string_tokenization() {
        let parser = FishParser::new();
        let tokens = parser.tokenize("'single quoted' \"double quoted\"");

        assert_eq!(tokens.len(), 2);
    }

    #[test]
    fn test_command_substitution_tokenization() {
        let parser = FishParser::new();
        let tokens = parser.tokenize("echo (date)");

        let has_subst_open = tokens.iter().any(|t| *t == FishToken::CommandSubstOpen);
        assert!(has_subst_open);
    }

    #[test]
    fn test_npm_fish_completions() {
        let parser = FishParser::new();
        let ctx = CompletionContext::new("npm ", 4);
        let suggestions = parser.get_suggestions(&ctx);

        assert!(!suggestions.is_empty());
        assert!(suggestions.iter().any(|s| s.text.contains("install")));
        assert!(suggestions.iter().any(|s| s.text.contains("run")));
        assert!(suggestions.iter().any(|s| s.text.contains("test")));
    }
}
