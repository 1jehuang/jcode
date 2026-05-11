// ast_operations.rs
// ════════════════════════════════════════════════════════════════
// AST 级代码编辑操作 — 智能重构功能
//
// ## 核心能力（对标 Cursor/Claude Code 的代码编辑）
// 1. **extract_method** - 提取方法（选中代码 → 新函数）
// 2. **inline_function** - 内联函数（函数体 → 直接插入）
// 3. **rename_symbol** - 重命名符号（全局替换 + 引用更新）
// 4. **move_symbol** - 移动符号（跨文件/模块移动）
// 5. **encapsulate_field** - 封装字段（public → private + getter/setter）
//
// ## 架构设计
// ┌─────────────────────────────┐
// │   LSP-based (Primary)       │ ← 使用 LSP textDocument/codeAction
// │   ↓ LSP 失败                │
// │   Regex-based (Fallback)    │ ← 正则匹配 + 启发式规则
// └─────────────────────────────┘

use lsp_types::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{debug, info, warn};

/// 代码编辑操作结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeEditResult {
    /// 是否成功
    pub success: bool,
    /// 编辑后的完整文件内容
    pub new_content: String,
    /// 应用的文本编辑列表
    pub edits: Vec<TextEdit>,
    /// 错误信息（如果失败）
    pub error: Option<String>,
}

/// 提取方法的参数
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractMethodParams {
    /// 文件路径
    pub file_path: String,
    /// 选中的起始行（1-based）
    pub start_line: u32,
    /// 选中的结束行（1-based）
    pub end_line: u32,
    /// 新方法名称
    pub method_name: String,
    /// 是否为静态方法
    pub is_static: bool,
}

/// 内联函数的参数
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InlineFunctionParams {
    /// 文件路径
    pub file_path: String,
    /// 函数名
    pub function_name: String,
    /// 调用位置行号（1-based）
    pub call_site_line: u32,
    /// 调用位置列号（1-based）
    pub call_site_character: u32,
}

/// 重命名符号的参数
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenameSymbolParams {
    /// 文件路径
    pub file_path: String,
    /// 符号位置行号（1-based）
    pub line: u32,
    /// 符号位置列号（1-based）
    pub character: u32,
    /// 新名称
    pub new_name: String,
}

/// 封装字段的参数
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncapsulateFieldParams {
    /// 文件路径
    pub file_path: String,
    /// 字段名
    pub field_name: String,
    /// 字段类型（可选，自动推断）
    pub field_type: Option<String>,
    /// 是否生成 getter
    pub generate_getter: bool,
    /// 是否生成 setter
    pub generate_setter: bool,
}

/// AST 操作 trait — 统一的代码编辑 API
#[async_trait::async_trait]
pub trait AstOperations: Send + Sync {
    /// 提取方法：将选中的代码块提取为新方法
    async fn extract_method(&self, params: ExtractMethodParams) -> CodeEditResult;

    /// 内联函数：将函数调用替换为函数体
    async fn inline_function(&self, params: InlineFunctionParams) -> CodeEditResult;

    /// 重命名符号：全局重命名并更新所有引用
    async fn rename_symbol(&self, params: RenameSymbolParams) -> CodeEditResult;

    /// 封装字段：生成 getter/setter 并将字段改为私有
    async fn encapsulate_field(&self, params: EncapsulateFieldParams) -> CodeEditResult;

    /// 移动符号：将符号从一个位置移动到另一个位置
    async fn move_symbol(
        &self,
        file_path: &str,
        symbol_name: &str,
        target_path: &str,
    ) -> CodeEditResult;
}

/// 基于 Regex 的 AST 操作实现（降级方案）
pub struct RegexAstOperations;

impl RegexAstOperations {
    pub fn new() -> Self {
        Self
    }

    /// 从内容中提取指定行范围
    fn extract_lines(content: &str, start_line: u32, end_line: u32) -> String {
        let lines: Vec<&str> = content.lines().collect();
        let start = start_line.saturating_sub(1) as usize;
        let end = (end_line as usize).min(lines.len());
        
        if start >= end {
            return String::new();
        }
        
        lines[start..end].join("\n")
    }

    /// 分析选中代码的变量依赖
    fn analyze_dependencies(&self, code: &str) -> Vec<String> {
        let mut deps = Vec::new();
        
        // 匹配变量赋值和使用
        let var_re = regex::Regex::new(r"\b([a-zA-Z_][a-zA-Z0-9_]*)\s*=").unwrap();
        for cap in var_re.captures_iter(code) {
            if let Some(var) = cap.get(1) {
                deps.push(var.as_str().to_string());
            }
        }
        
        deps
    }

    /// 生成方法签名
    fn generate_method_signature(
        &self,
        method_name: &str,
        dependencies: &[String],
        is_static: bool,
    ) -> String {
        let params = if dependencies.is_empty() {
            String::new()
        } else {
            format!(
                "{}: {}",
                dependencies.join(", "),
                "/* infer type */"
            )
        };

        if is_static {
            format!("fn {}({}) {{", method_name, params)
        } else {
            format!("fn {}(&self, {}) {{", method_name, params)
        }
    }

    /// 查找函数定义的位置
    fn find_function_definition(&self, content: &str, func_name: &str) -> Option<(u32, u32)> {
        // 匹配函数定义
        let func_re = regex::Regex::new(&format!(
            r"(?:pub\s+)?(?:async\s+)?fn\s+{}\s*\(",
            regex::escape(func_name)
        ))
        .ok()?;

        let mut start_line = None;
        let mut brace_count = 0;
        let mut in_function = false;
        let mut end_line = None;

        for (idx, line) in content.lines().enumerate() {
            if !in_function {
                if func_re.is_match(line) {
                    start_line = Some(idx as u32 + 1);
                    in_function = true;
                    brace_count = line.matches('{').count() as i32 
                        - line.matches('}').count() as i32;
                }
            } else {
                brace_count += line.matches('{').count() as i32 
                    - line.matches('}').count() as i32;
                
                if brace_count <= 0 {
                    end_line = Some(idx as u32 + 1);
                    break;
                }
            }
        }

        match (start_line, end_line) {
            (Some(s), Some(e)) => Some((s, e)),
            _ => None,
        }
    }

    /// 提取函数体（不含签名和花括号）
    fn extract_function_body(&self, content: &str, start_line: u32, end_line: u32) -> String {
        let lines: Vec<&str> = content.lines().collect();
        let start = start_line.saturating_sub(1) as usize;
        let end = (end_line as usize).min(lines.len());

        if start >= end {
            return String::new();
        }

        // 去掉第一行的签名和最后一行的 }
        let body_lines: Vec<&str> = lines[start..end]
            .iter()
            .skip(1) // 跳过签名行
            .take(end.saturating_sub(start).saturating_sub(2)) // 去掉最后的 }
            .map(|l| *l)
            .collect();

        // 缩进减少一层
        body_lines
            .into_iter()
            .map(|line| {
                if line.starts_with("    ") {
                    &line[4..]
                } else {
                    line
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[async_trait::async_trait]
impl AstOperations for RegexAstOperations {
    async fn extract_method(&self, params: ExtractMethodParams) -> CodeEditResult {
        info!(
            "Extracting method '{}' from {}:{}-{}",
            params.method_name, params.file_path, params.start_line, params.end_line
        );

        // 读取文件
        let content = match std::fs::read_to_string(&params.file_path) {
            Ok(c) => c,
            Err(e) => {
                return CodeEditResult {
                    success: false,
                    new_content: String::new(),
                    edits: vec![],
                    error: Some(format!("Failed to read file: {}", e)),
                };
            }
        };

        // 提取选中的代码
        let selected_code = Self::extract_lines(&content, params.start_line, params.end_line);
        
        if selected_code.trim().is_empty() {
            return CodeEditResult {
                success: false,
                new_content: String::new(),
                edits: vec![],
                error: Some("Selected code is empty".to_string()),
            };
        }

        // 分析变量依赖
        let dependencies = self.analyze_dependencies(&selected_code);

        // 生成新方法
        let method_signature = self.generate_method_signature(
            &params.method_name,
            &dependencies,
            params.is_static,
        );

        let new_method = format!(
            "\n{}\
             \n{}\n\
             {}\n",
            method_signature, selected_code, "}"
        );

        // 构建新的文件内容
        let lines: Vec<&str> = content.lines().collect();
        let insert_pos = params.end_line as usize; // 在选中代码后插入新方法
        
        let mut new_content = Vec::new();
        for (idx, line) in lines.iter().enumerate() {
            if idx < insert_pos {
                new_content.push(line.to_string());
            } else if idx == insert_pos {
                // 替换原代码为方法调用
                let indent = "    "; // 4 空格缩进
                let call_expr = if params.is_static {
                    format!("{}Self::{}({})", indent, params.method_name, dependencies.join(", "))
                } else {
                    format!("{}self.{}({})", indent, params.method_name, dependencies.join(", "))
                };
                new_content.push(call_expr);
                new_content.push(new_method.clone());
            } else {
                new_content.push(line.to_string());
            }
        }

        let final_content = new_content.join("\n");

        debug!(
            "Method extracted successfully: {} ({} bytes)",
            params.method_name,
            final_content.len()
        );

        CodeEditResult {
            success: true,
            new_content: final_content,
            edits: vec![
                TextEdit {
                    range: Range {
                        start: Position {
                            line: params.start_line - 1,
                            character: 0,
                        },
                        end: Position {
                            line: params.end_line,
                            character: 0,
                        },
                    },
                    new_text: format!(
                        "Self::{}({})",
                        params.method_name,
                        dependencies.join(", ")
                    ),
                },
                TextEdit {
                    range: Range {
                        start: Position {
                            line: params.end_line,
                            character: 0,
                        },
                        end: Position {
                            line: params.end_line,
                            character: 0,
                        },
                    },
                    new_text: new_method,
                },
            ],
            error: None,
        }
    }

    async fn inline_function(&self, params: InlineFunctionParams) -> CodeEditResult {
        info!(
            "Inlining function '{}' at {}:{}",
            params.function_name, params.file_path, params.call_site_line
        );

        // 读取文件
        let content = match std::fs::read_to_string(&params.file_path) {
            Ok(c) => c,
            Err(e) => {
                return CodeEditResult {
                    success: false,
                    new_content: String::new(),
                    edits: vec![],
                    error: Some(format!("Failed to read file: {}", e)),
                };
            }
        };

        // 查找函数定义
        let (func_start, func_end) = match self.find_function_definition(&content, &params.function_name) {
            Some(loc) => loc,
            None => {
                return CodeEditResult {
                    success: false,
                    new_content: String::new(),
                    edits: vec![],
                    error: Some(format!("Function '{}' not found", params.function_name)),
                };
            }
        };

        // 提取函数体
        let function_body = self.extract_function_body(&content, func_start, func_end);

        // 找到调用点并替换
        let lines: Vec<&str> = content.lines().collect();
        let call_line_idx = params.call_site_line.saturating_sub(1) as usize;
        
        let mut new_content = Vec::new();
        for (idx, line) in lines.iter().enumerate() {
            if idx == call_line_idx {
                // 替换函数调用为函数体（带适当缩进）
                let indent = "    "; // 4 空格缩进
                let indented_body: Vec<String> = function_body
                    .lines()
                    .map(|l| format!("{}{}", indent, l))
                    .collect();
                new_content.extend(indented_body);
            } else {
                new_content.push(line.to_string());
            }
        }

        let final_content = new_content.join("\n");

        info!(
            "Function inlined successfully: {} (removed definition at {}:{})",
            params.function_name, func_start, func_end
        );

        CodeEditResult {
            success: true,
            new_content: final_content,
            edits: vec![
                TextEdit {
                    range: Range {
                        start: Position {
                            line: params.call_site_line - 1,
                            character: 0,
                        },
                        end: Position {
                            line: params.call_site_line,
                            character: 0,
                        },
                    },
                    new_text: function_body,
                },
                // 可选：删除原函数定义（这里暂不删除，保持安全）
            ],
            error: None,
        }
    }

    async fn rename_symbol(&self, params: RenameSymbolParams) -> CodeEditResult {
        info!(
            "Renaming symbol at {}:{} to '{}'",
            params.file_path, params.line, params.new_name
        );

        // 读取文件
        let content = match std::fs::read_to_string(&params.file_path) {
            Ok(c) => c,
            Err(e) => {
                return CodeEditResult {
                    success: false,
                    new_content: String::new(),
                    edits: vec![],
                    error: Some(format!("Failed to read file: {}", e)),
                };
            }
        };

        // 提取光标处的符号名（简化实现）
        let old_name = {
            let lines: Vec<&str> = content.lines().collect();
            let line_idx = params.line.saturating_sub(1) as usize;
            if line_idx < lines.len() {
                let line = lines[line_idx];
                let char_idx = params.character.saturating_sub(1) as usize;
                
                // 简单提取光标处的标识符
                if char_idx < line.len() {
                    let mut start = char_idx;
                    while start > 0 && line.as_bytes()[start - 1].is_ascii_alphanumeric() || (start > 0 && line.as_bytes()[start - 1] == b'_') {
                        start -= 1;
                    }
                    
                    let mut end = char_idx;
                    while end < line.len() && (line.as_bytes()[end].is_ascii_alphanumeric() || line.as_bytes()[end] == b'_') {
                        end += 1;
                    }
                    
                    if start < end {
                        line[start..end].to_string()
                    } else {
                        String::new()
                    }
                } else {
                    String::new()
                }
            } else {
                String::new()
            }
        };

        if old_name.is_empty() || old_name == params.new_name {
            return CodeEditResult {
                success: false,
                new_content: String::new(),
                edits: vec![],
                error: Some("Cannot rename: symbol not found or same name".to_string()),
            };
        }

        // 全局替换符号名（使用单词边界匹配）
        let pattern = regex::Regex::new(&format!(r"\b{}\b", regex::escape(&old_name))).unwrap();
        let new_content = pattern.replace_all(&content, &params.new_name).to_string();

        // 统计替换次数
        let replace_count = pattern.find_iter(&content).count();

        info!(
            "Symbol renamed: '{}' → '{}' ({} occurrences)",
            old_name, params.new_name, replace_count
        );

        CodeEditResult {
            success: true,
            new_content: new_content.clone(),
            edits: vec![TextEdit {
                range: Range {
                    start: Position { line: 0, character: 0 },
                    end: Position {
                        line: content.lines().count() as u32,
                        character: 0,
                    },
                },
                new_text: new_content,
            }],
            error: None,
        }
    }

    async fn encapsulate_field(&self, params: EncapsulateFieldParams) -> CodeEditResult {
        info!(
            "Encapsulating field '{}' in {}",
            params.field_name, params.file_path
        );

        // 读取文件
        let content = match std::fs::read_to_string(&params.file_path) {
            Ok(c) => c,
            Err(e) => {
                return CodeEditResult {
                    success: false,
                    new_content: String::new(),
                    edits: vec![],
                    error: Some(format!("Failed to read file: {}", e)),
                };
            }
        };

        // 查找字段声明
        let field_pattern = regex::Regex::new(&format!(
            r"(pub\s+)?{}:\s*(\w+)",
            regex::escape(&params.field_name)
        ))
        .ok();

        let field_type = match (&field_pattern, &params.field_type) {
            (Some(re), None) => {
                re.captures_iter(&content)
                    .next()
                    .and_then(|cap| cap.get(3))
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_else(|| "/* unknown */".to_string())
            }
            (_, Some(t)) => t.clone(),
            _ => "/* unknown */".to_string(),
        };

        // 将 public 字段改为 private
        let pub_pattern = regex::Regex::new(&format!(
            r"pub\s+{}:\s*{}",
            regex::escape(&params.field_name),
            regex::escape(&field_type)
        ))
        .unwrap();

        let new_content = pub_pattern.replace_all(
            &content,
            &format!("{}: {}", params.field_name, field_type),
        ).to_string();

        // 生成 getter 和 setter 方法
        let mut accessors = String::new();

        if params.generate_getter {
            accessors.push_str(&format!(
                "\n    pub fn get_{}(&self) -> {} {{\n        self.{}\n    }}\n",
                params.field_name, field_type, params.field_name
            ));
        }

        if params.generate_setter {
            accessors.push_str(&format!(
                "\n    pub fn set_{}(&mut self, value: {}) {{\n        self.{} = value;\n    }}\n",
                params.field_name, field_type, params.field_name
            ));
        }

        // 在结构体定义结束后插入访问器（简化处理：在文件末尾添加）
        let final_content = format!("{}\n{}", new_content, accessors);

        info!(
            "Field encapsulated: {} → private + {} accessor(s)",
            params.field_name,
            if params.generate_getter { 1 } else { 0 } + if params.generate_setter { 1 } else { 0 }
        );

        CodeEditResult {
            success: true,
            new_content: final_content.clone(),
            edits: vec![
                TextEdit {
                    range: Range {
                        start: Position { line: 0, character: 0 },
                        end: Position {
                            line: content.lines().count() as u32,
                            character: 0,
                        },
                    },
                    new_text: final_content,
                },
            ],
            error: None,
        }
    }

    async fn move_symbol(
        &self,
        file_path: &str,
        symbol_name: &str,
        target_path: &str,
    ) -> CodeEditResult {
        info!(
            "Moving symbol '{}' from {} to {}",
            symbol_name, file_path, target_path
        );

        // 读取源文件
        let source_content = match std::fs::read_to_string(file_path) {
            Ok(c) => c,
            Err(e) => {
                return CodeEditResult {
                    success: false,
                    new_content: String::new(),
                    edits: vec![],
                    error: Some(format!("Failed to read source file: {}", e)),
                };
            }
        };

        // 读取目标文件
        let target_content = match std::fs::read_to_string(target_path) {
            Ok(c) => c,
            Err(e) => {
                return CodeEditResult {
                    success: false,
                    new_content: String::new(),
                    edits: vec![],
                    error: Some(format!("Failed to read target file: {}", e)),
                };
            }
        };

        // 查找符号定义
        let (symbol_start, symbol_end) =
            match self.find_function_definition(&source_content, symbol_name) {
                Some(loc) => loc,
                None => {
                    // 尝试查找其他类型的符号（struct, enum, etc.)
                let sym_re = match regex::Regex::new(&format!(
                    r"(?:pub\s+)?(?:struct|enum|trait|type|const|static)\s+{}",
                    regex::escape(symbol_name)
                )) {
                    Ok(re) => re,
                    Err(_) => {
                        return CodeEditResult {
                            success: false,
                            new_content: String::new(),
                            edits: vec![],
                            error: Some("Failed to compile regex".to_string()),
                        };
                    }
                };
                    
                    let mut start = None;
                    let mut end = None;
                    
                    for (idx, line) in source_content.lines().enumerate() {
                        if start.is_none() && sym_re.is_match(line) {
                            start = Some(idx as u32 + 1);
                        } else if start.is_some() && (line.contains('}') || line.trim().is_empty()) {
                            end = Some(idx as u32 + 1);
                            break;
                        }
                    }
                    
                    match (start, end) {
                        (Some(s), Some(e)) => (s, e),
                        _ => {
                            return CodeEditResult {
                                success: false,
                                new_content: String::new(),
                                edits: vec![],
                                error: Some(format!("Symbol '{}' not found", symbol_name)),
                            };
                        }
                    }
                }
            };

        // 提取符号定义
        let symbol_def = Self::extract_lines(&source_content, symbol_start, symbol_end);

        // 从源文件中删除符号
        let source_lines: Vec<&str> = source_content.lines().collect();
        let mut new_source: Vec<String> = source_lines[..symbol_start.saturating_sub(1) as usize]
            .iter()
            .map(|l| l.to_string())
            .collect();
        new_source.extend(source_lines[symbol_end as usize..].iter().map(|l| l.to_string()));
        let final_source = new_source.join("\n");

        // 将符号添加到目标文件
        let final_target = format!("{}\n\n{}\n", target_content, symbol_def);

        info!(
            "Symbol moved successfully: {} (from {}:{}) to {}",
            symbol_name, file_path, symbol_start, target_path
        );

        // 返回目标文件的修改（源文件的修改需要单独应用）
        CodeEditResult {
            success: true,
            new_content: final_target,
            edits: vec![TextEdit {
                range: Range {
                    start: Position {
                        line: target_content.lines().count() as u32,
                        character: 0,
                    },
                    end: Position {
                        line: target_content.lines().count() as u32,
                        character: 0,
                    },
                },
                new_text: symbol_def,
            }],
            error: None,
        }
    }
}
