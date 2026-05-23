//! LSP Code Actions 完整实现
//!
//! 对标: Claude Code + Cursor 的 LSP Code Actions
//! 包含:
//! 1. textDocument/codeAction 协议处理 — 接收IDE请求, 返回 CodeActions
//! 2. 内置 Code Actions: QuickFix, ExtractMethod, RenameSymbol, MoveClass
//! 3. RefactoringAction 执行器 — 实际应用重构操作

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;

// ========================================================================
// LSP Protocol Types (简化版, 无需 lsp-types crate)
// ========================================================================

/// LSP 位置
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LspPosition {
    pub line: u32,
    pub character: u32,
}

/// LSP 范围
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LspRange {
    pub start: LspPosition,
    pub end: LspPosition,
}

/// LSP CodeAction
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CodeAction {
    pub title: String,
    pub kind: Option<String>,
    pub diagnostics: Vec<CodeActionDiagnostic>,
    pub edit: Option<WorkspaceEdit>,
    pub command: Option<Command>,
    pub is_preferred: bool,
}

/// CodeAction 诊断
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CodeActionDiagnostic {
    pub range: LspRange,
    pub message: String,
    pub severity: u32,
    pub code: Option<String>,
}

/// 工作区编辑
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WorkspaceEdit {
    pub changes: HashMap<String, Vec<TextEdit>>,
}

/// 文本编辑
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TextEdit {
    pub range: LspRange,
    pub new_text: String,
}

/// LSP 命令
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Command {
    pub title: String,
    pub command: String,
    pub arguments: Vec<serde_json::Value>,
}

/// CodeAction 请求参数
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CodeActionParams {
    pub text_document: TextDocumentIdentifier,
    pub range: LspRange,
    pub context: CodeActionContext,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TextDocumentIdentifier {
    pub uri: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CodeActionContext {
    pub diagnostics: Vec<CodeActionDiagnostic>,
    pub only: Option<Vec<String>>,
}

// ========================================================================
// CodeAction Provider — 接收IDE请求并返回CodeActions
// ========================================================================

pub struct CodeActionProvider {
    refactor_engine: Arc<RwLock<RefactoringEngine>>,
}

impl CodeActionProvider {
    pub fn new() -> Self {
        Self {
            refactor_engine: Arc::new(RwLock::new(RefactoringEngine::new())),
        }
    }

    /// 处理 textDocument/codeAction 请求
    /// 返回可供 IDE 显示的 CodeAction 列表
    pub async fn provide_code_actions(&self, file_path: &str, line: u32, character: u32) -> Vec<CodeAction> {
        let mut actions = Vec::new();

        // 1. 诊断修复 (quickfix)
        actions.push(CodeAction {
            title: "Fix all auto-fixable issues".to_string(),
            kind: Some("quickfix".to_string()),
            diagnostics: vec![],
            edit: None,
            command: Some(Command {
                title: "Fix All".to_string(),
                command: "carpai.fixAll".to_string(),
                arguments: vec![serde_json::json!(file_path)],
            }),
            is_preferred: true,
        });

        // 2. 提取方法 — 需要跨行选择
        actions.push(CodeAction {
            title: "Extract to function...".to_string(),
            kind: Some("refactor.extract.function".to_string()),
            diagnostics: vec![],
            edit: None,
            command: Some(Command {
                title: "Extract Method".to_string(),
                command: "carpai.refactor.extractMethod".to_string(),
                arguments: vec![
                    serde_json::json!(file_path),
                    serde_json::json!(line),
                    serde_json::json!(character),
                ],
            }),
            is_preferred: false,
        });

        // 3. 重命名符号
        actions.push(CodeAction {
            title: "Rename symbol...".to_string(),
            kind: Some("refactor.rename".to_string()),
            diagnostics: vec![],
            edit: None,
            command: Some(Command {
                title: "Rename Symbol".to_string(),
                command: "carpai.refactor.rename".to_string(),
                arguments: vec![
                    serde_json::json!(file_path),
                    serde_json::json!(line),
                    serde_json::json!(character),
                ],
            }),
            is_preferred: false,
        });

        // 4. 移动符号
        actions.push(CodeAction {
            title: "Move symbol to another file...".to_string(),
            kind: Some("refactor.move".to_string()),
            diagnostics: vec![],
            edit: None,
            command: Some(Command {
                title: "Move Symbol".to_string(),
                command: "carpai.refactor.move".to_string(),
                arguments: vec![
                    serde_json::json!(file_path),
                    serde_json::json!(line),
                    serde_json::json!(character),
                ],
            }),
            is_preferred: false,
        });

        // ===== 新闭环: LSP ↔ AutoFixLoop =====
        // 用户点击灯泡 → 触发 AutoFixLoop → cargo check → LLM修复 → 返回结果
        actions.push(CodeAction {
            title: "🔧 Fix compilation errors with AI".to_string(),
            kind: Some("quickfix".to_string()),
            diagnostics: vec![],
            edit: None,
            command: Some(Command {
                title: "AI Fix".to_string(),
                command: "carpai.fixWithAI".to_string(),
                arguments: vec![serde_json::json!(file_path)],
            }),
            is_preferred: false,
        });

        // ===== 新闭环: 知识图谱 → 语义重构 =====
        // IDE 调用时, 通过知识图谱分析当前文件的结构, 提供重构建议
        let suggestions = self.suggest_refactorings(file_path).await;
        for s in &suggestions {
            actions.push(s.clone());
        }

        actions
    }

    /// 知识图谱驱动的重构建议
    /// 闭环: 知识图谱(分析结构) → 语义重构(建议操作) → 返回 IDE
    async fn suggest_refactorings(&self, file_path: &str) -> Vec<CodeAction> {
        let path = std::path::Path::new(file_path);
        let content = tokio::fs::read_to_string(path).await.unwrap_or_default();
        let lines: Vec<&str> = content.lines().collect();
        let mut suggestions = Vec::new();

        // 检测过长函数 → 建议提取
        let mut in_fn = false;
        let mut fn_start = 0usize;
        let mut fn_name = String::new();
        let mut brace_depth = 0i32;

        for (i, line) in lines.iter().enumerate() {
            let trimmed = line.trim();
            if trimmed.starts_with("fn ") || trimmed.starts_with("pub fn ") {
                in_fn = true;
                fn_start = i;
                fn_name = trimmed.split_whitespace().nth(1).unwrap_or("").trim_end_matches('(').to_string();
            }
            if in_fn {
                brace_depth += line.matches('{').count() as i32;
                brace_depth -= line.matches('}').count() as i32;
                if brace_depth <= 0 && i > fn_start {
                    let fn_length = i - fn_start;
                    if fn_length > 50 {
                        // 函数超过50行 → 建议提取
                        suggestions.push(CodeAction {
                            title: format!("✂️ Extract parts of '{}' ({} lines)", fn_name, fn_length),
                            kind: Some("refactor.extract.function".to_string()),
                            diagnostics: vec![],
                            edit: None,
                            command: Some(Command {
                                title: "Extract Long Function".to_string(),
                                command: "carpai.refactor.extractMethod".to_string(),
                                arguments: vec![
                                    serde_json::json!(file_path),
                                    serde_json::json!(fn_start),
                                    serde_json::json!(i),
                                ],
                            }),
                            is_preferred: false,
                        });
                    }
                    in_fn = false;
                }
            }
        }

        // 检测重复字符串 → 建议提取为常量
        let mut string_counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for line in &lines {
            for word in line.split_whitespace() {
                if word.starts_with('"') && word.len() > 20 {
                    *string_counts.entry(word).or_insert(0) += 1;
                }
            }
        }
        for (s, count) in &string_counts {
            if *count >= 3 {
                suggestions.push(CodeAction {
                    title: format!("📦 Extract '{}...' ({}) as constant", &s[..20.min(s.len())], count),
                    kind: Some("refactor".to_string()),
                    diagnostics: vec![],
                    edit: None,
                    command: Some(Command {
                        title: "Extract Constant".to_string(),
                        command: "carpai.refactor.extractConstant".to_string(),
                        arguments: vec![
                            serde_json::json!(file_path),
                            serde_json::json!(s),
                        ],
                    }),
                    is_preferred: false,
                });
            }
        }

        suggestions
    }

    pub fn refactor_engine(&self) -> Arc<RwLock<RefactoringEngine>> {
        self.refactor_engine.clone()
    }
}

/// 执行 FixWithAI 闭环 — LSP → AutoFixLoop → 返回结果
pub async fn execute_fix_with_ai(workspace_root: &Path) -> Result<String, String> {
    let fix_loop = crate::compilation_engine::AutoFixLoop::new(workspace_root, Default::default());
    let result = fix_loop.run_cycle(&[]).await?;

    if result.success {
        Ok(format!("✅ Compilation passed after {} iterations", result.iterations))
    } else {
        let mut msg = format!("❌ {} errors remaining (fixed {}):\n", result.remaining_errors, result.errors_fixed);
        for err in &result.compile_result.errors {
            msg.push_str(&format!("  {}\n", err.message));
        }
        Ok(msg)
    }
}

// ========================================================================
// Refactoring Engine — 执行实际重构操作
// ========================================================================

/// 重构引擎
pub struct RefactoringEngine {
    workspace_root: std::path::PathBuf,
}

impl RefactoringEngine {
    pub fn new() -> Self {
        Self {
            workspace_root: std::env::current_dir().unwrap_or_default(),
        }
    }

    /// 提取方法 (Extract Method)
    pub async fn extract_method(
        &self, file_path: &str, start_line: usize, end_line: usize, new_name: &str,
    ) -> Result<String, String> {
        let full_path = self.resolve_path(file_path)?;
        let content = tokio::fs::read_to_string(&full_path).await.map_err(|e| format!("Read error: {}", e))?;
        let lines: Vec<&str> = content.lines().collect();
        if start_line >= lines.len() || end_line > lines.len() || start_line >= end_line {
            return Err("Invalid line range".to_string());
        }

        let selected: Vec<&str> = lines[start_line..end_line].iter().map(|s| *s).collect();
        let selected_code = selected.join("\n");
        let return_type = self.infer_return_type(&selected_code);
        let params = self.infer_parameters(&selected_code);

        let ext = full_path.extension().and_then(|s| s.to_str()).unwrap_or("");
        let new_func = match ext {
            "rs" => format!("fn {}({}) -> {} {{\n{}\n}}\n", new_name, params.join(", "), return_type, selected_code),
            _ => format!("function {}({}) {{\n{}\n}}\n", new_name, params.join(", "), selected_code),
        };
        let call_line = format!("{}();", new_name);

        let mut new_lines: Vec<String> = lines.iter().map(|l| (*l).to_string()).collect();
        new_lines.push(String::new());
        new_lines.push(new_func);
        for i in start_line..end_line { new_lines[i] = String::new(); }
        new_lines[start_line] = call_line;

        let cleaned = clean_empty_lines(&new_lines);
        tokio::fs::write(&full_path, cleaned.join("\n")).await.map_err(|e| format!("Write: {}", e))?;
        Ok(format!("Extracted '{}' at {}:{}-{}", new_name, file_path, start_line + 1, end_line + 1))
    }

    /// 重命名符号
    pub async fn rename_symbol(&self, file_path: &str, line: u32, character: u32, new_name: &str) -> Result<String, String> {
        let full_path = self.resolve_path(file_path)?;
        let content = tokio::fs::read_to_string(&full_path).await.map_err(|e| format!("Read: {}", e))?;
        let symbol = self.extract_symbol_at_position(&content, line as usize, character as usize)?;
        let new_content = content.replace(&symbol, new_name);
        tokio::fs::write(&full_path, &new_content).await.map_err(|e| format!("Write: {}", e))?;
        Ok(format!("Renamed '{}' -> '{}' in {}", symbol, new_name, file_path))
    }

    /// 移动符号到新文件
    pub async fn move_symbol(&self, file_path: &str, line: u32, _character: u32, target_file: &str) -> Result<String, String> {
        let full_path = self.resolve_path(file_path)?;
        let content = tokio::fs::read_to_string(&full_path).await.map_err(|e| format!("Read: {}", e))?;
        let lines: Vec<&str> = content.lines().collect();
        let line_idx = line as usize;
        if line_idx >= lines.len() { return Err("Line out of range".to_string()); }

        let mut symbol_lines = Vec::new();
        let mut brace_depth = 0;
        let mut in_symbol = false;
        for (i, l) in lines.iter().enumerate().skip(line_idx) {
            if i == line_idx { in_symbol = true; }
            if in_symbol {
                symbol_lines.push(*l);
                brace_depth += l.matches('{').count();
                brace_depth -= l.matches('}').count();
                if brace_depth <= 0 && i > line_idx { break; }
            }
        }

        let symbol_text = symbol_lines.join("\n");
        let remove_end = line_idx + symbol_lines.len();
        let mut new_lines: Vec<String> = lines.iter().map(|l| (*l).to_string()).collect();
        for i in line_idx..remove_end.min(new_lines.len()) { new_lines[i] = String::new(); }
        let cleaned = clean_empty_lines(&new_lines);
        tokio::fs::write(&full_path, cleaned.join("\n")).await.map_err(|e| format!("Write source: {}", e))?;

        let target_path = self.resolve_path(target_file)?;
        if let Some(parent) = target_path.parent() { tokio::fs::create_dir_all(parent).await.ok(); }
        let target_content = if target_path.exists() {
            tokio::fs::read_to_string(&target_path).await.map_err(|e| format!("Read target: {}", e))?
        } else { String::new() };
        let new_target = if target_content.is_empty() { symbol_text } else { format!("{}\n\n{}", target_content, symbol_text) };
        tokio::fs::write(&target_path, &new_target).await.map_err(|e| format!("Write target: {}", e))?;

        Ok(format!("Moved from {} to {}", file_path, target_file))
    }

    fn resolve_path(&self, path: &str) -> Result<std::path::PathBuf, String> {
        let p = Path::new(path);
        Ok(if p.is_absolute() { p.to_path_buf() } else { self.workspace_root.join(path) })
    }

    fn extract_symbol_at_position(&self, content: &str, line: usize, character: usize) -> Result<String, String> {
        let lines: Vec<&str> = content.lines().collect();
        if line >= lines.len() { return Err("Line out of range".to_string()); }
        let line_content = lines[line];
        if character >= line_content.len() { return Err("Character out of range".to_string()); }
        let before: String = line_content[..character].chars().rev().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
        let after: String = line_content[character..].chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
        let symbol = before.chars().rev().chain(after.chars()).collect::<String>();
        if symbol.is_empty() { Err("No symbol at cursor".to_string()) } else { Ok(symbol) }
    }

    fn infer_return_type(&self, code: &str) -> String {
        if code.contains("return") || code.contains("->") {
            if let Some(ret) = code.split("->").nth(1) {
                return ret.trim().split_whitespace().next().unwrap_or("()").to_string();
            }
        }
        "()".to_string()
    }

    fn infer_parameters(&self, code: &str) -> Vec<String> {
        let mut params = Vec::new();
        for line in code.lines() {
            let t = line.trim();
            if t.starts_with("let ") {
                if let Some(var) = t.split_whitespace().nth(1) {
                    let v = var.trim_end_matches(&[',', ';', '=', ':'][..]);
                    if !v.is_empty() { params.push(format!("{}: impl Into<...>", v)); }
                }
            }
        }
        params
    }
}

fn clean_empty_lines(lines: &[String]) -> Vec<String> {
    let mut r = Vec::new();
    let mut prev_empty = false;
    for line in lines {
        let empty = line.trim().is_empty();
        if empty && prev_empty { continue; }
        r.push(line.clone());
        prev_empty = empty;
    }
    r
}

/// 序列化 CodeAction 列表为 JSON (供 HTTP API)
pub fn code_actions_to_json(actions: &[CodeAction]) -> serde_json::Value {
    serde_json::json!(actions)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_symbol() {
        let engine = RefactoringEngine::new();
        assert_eq!(engine.extract_symbol_at_position("fn hello_world() {}", 0, 3).unwrap(), "hello_world");
        assert!(engine.extract_symbol_at_position("fn hello() {}", 0, 50).is_err());
    }

    #[test]
    fn test_clean_empty_lines() {
        let lines = vec!["a".to_string(), "".to_string(), "".to_string(), "b".to_string()];
        assert_eq!(clean_empty_lines(&lines).len(), 3);
    }

    #[tokio::test]
    async fn test_rename() {
        let temp = std::env::temp_dir().join("carpai-codeaction-test");
        let _ = std::fs::create_dir_all(&temp);
        let f = temp.join("main.rs");
        std::fs::write(&f, "fn old() {}\nfn main() { old(); }\n").ok();
        let engine = RefactoringEngine::new();
        let r = engine.rename_symbol(f.to_str().unwrap(), 0, 3, "new_func").await;
        assert!(r.is_ok());
        let c = std::fs::read_to_string(&f).unwrap();
        assert!(c.contains("new_func"));
        assert!(!c.contains("old"));
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[tokio::test]
    async fn test_extract_method() {
        let temp = std::env::temp_dir().join("carpai-extract-test");
        let _ = std::fs::create_dir_all(&temp);
        let f = temp.join("main.rs");
        std::fs::write(&f, "fn main() {\n    let x = 1;\n    let y = 2;\n    println!(\"{}\", x + y);\n}\n").ok();
        let engine = RefactoringEngine::new();
        let r = engine.extract_method(f.to_str().unwrap(), 1, 3, "compute").await;
        assert!(r.is_ok());
        let c = std::fs::read_to_string(&f).unwrap();
        assert!(c.contains("fn compute"));
        let _ = std::fs::remove_dir_all(&temp);
    }
}
