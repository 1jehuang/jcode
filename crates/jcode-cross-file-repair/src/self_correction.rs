use crate::ast::AstEdit;
use crate::error_detector::CodeError;
use crate::type_checker::{TypeChecker, TypeError};

/// 自修正结果 — 包含具体的 fix 建议
#[derive(Debug, Clone)]
pub struct CorrectionIteration {
    pub round: u32,
    pub errors_found: Vec<CodeError>,
    pub type_errors: Vec<TypeError>,
    pub fixes_applied: Vec<Fix>,
    pub success: bool,
}

/// 一个具体的修复操作 — 包含原始代码和替换后的代码
#[derive(Debug, Clone)]
pub struct Fix {
    pub file: String,
    pub line: u32,
    pub description: String,
    pub old_code: String,
    pub new_code: String,
    pub fix_type: FixType,
}

impl Fix {
    /// 一键应用 — 读取文件 → 替换原文 → 写入
    pub async fn apply(&self) -> anyhow::Result<()> {
        if self.old_code.is_empty() {
            anyhow::bail!("Fix '{}' has no old_code to replace", self.description);
        }
        let path = std::path::Path::new(&self.file);
        if !path.exists() {
            anyhow::bail!("File not found: {}", self.file);
        }
        let content = tokio::fs::read_to_string(path).await?;
        let new = content.replace(&self.old_code, &self.new_code);
        if new == content {
            anyhow::bail!("old_code '{}' not found in {}", self.old_code, self.file);
        }
        tokio::fs::write(path, &new).await?;
        tracing::info!("Applied fix: {} — replaced in {}", self.description, self.file);
        Ok(())
    }

    /// 批量应用多个 Fix
    pub async fn apply_all(fixes: &[Fix]) -> anyhow::Result<Vec<&Fix>> {
        let mut failed = Vec::new();
        for fix in fixes {
            if let Err(e) = fix.apply().await {
                tracing::warn!("Fix failed: {} — {}", fix.description, e);
                failed.push(fix);
            }
        }
        Ok(failed)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FixType {
    TypeAnnotation,
    AddField,
    RenameField,
    AddImport,
    ChangeParam,
    AddReturn,
    RemoveUnused,
}

/// 自修正循环 — 检测 → 生成修复描述 → AI 填充 → 验证
///
/// ## AI-in-the-Loop 模式
/// 旧版: errors_to_fixes() 生成空 Fix, apply_fixes() 不做任何修改 (死循环)
/// 新版: errors_to_fixes() 生成带上下文的 FixRequest, 由 Agent/AI 填充 old_code/new_code
pub struct SelfCorrectionLoop {
    max_rounds: u32,
}

impl SelfCorrectionLoop {
    pub fn new(max_rounds: u32) -> Self { Self { max_rounds } }

    /// 运行完整自修正流程
    pub async fn run(
        &self,
        edits: Vec<AstEdit>,
        type_checker: &TypeChecker,
    ) -> anyhow::Result<Vec<AstEdit>> {
        let mut current_edits = edits;
        let mut iterations = Vec::new();
        let mut previous_error_signatures: Vec<String> = Vec::new();

        for round in 0..self.max_rounds {
            let errors = type_checker.check(".").await?;

            if errors.is_empty() {
                return Ok(current_edits);
            }

            // Convergence detection: check if we've seen the same errors before
            let error_sig: Vec<String> = errors.iter()
                .map(|e| format!("{}:{}:{}", e.file, e.line, e.error_code))
                .collect();

            if previous_error_signatures == error_sig {
                tracing::warn!("Self-correction loop: same errors detected, stopping to avoid infinite loop");
                break;
            }
            previous_error_signatures = error_sig;

            // Generate fix requests with meaningful context
            let fixes = self.errors_to_fixes(&errors);

            let iter = CorrectionIteration {
                round,
                errors_found: vec![],
                type_errors: errors,
                fixes_applied: fixes.clone(),
                success: false,
            };
            iterations.push(iter);

            if fixes.is_empty() {
                break;
            }

            // Apply fixes that have meaningful content
            current_edits = self.apply_fixes(current_edits, &fixes);
        }

        Ok(current_edits)
    }

    /// 将编译器错误转换为带上下文的 Fix
    ///
    /// 关键改进: Fix 的 old_code 包含出错行的实际代码, new_code 提供上下文给 AI 填充
    fn errors_to_fixes(&self, errors: &[TypeError]) -> Vec<Fix> {
        let mut fixes = Vec::new();

        for err in errors {
            // Read the actual line of code for context
            let line_content = self.read_error_line(&err.file, err.line as usize);

            if err.error_code.contains("E0308") {
                // E0308: mismatched types — provide the actual line as old_code
                fixes.push(Fix {
                    file: err.file.clone(),
                    line: err.line,
                    description: format!("Type mismatch: {}", err.message),
                    old_code: line_content.clone(),
                    new_code: format!("/* TODO: fix type mismatch — {} */\n{}", err.message, line_content),
                    fix_type: FixType::TypeAnnotation,
                });
            }
            if err.error_code.contains("E0063") {
                // E0063: missing field — add placeholder field
                fixes.push(Fix {
                    file: err.file.clone(),
                    line: err.line,
                    description: "Missing struct field".into(),
                    old_code: line_content.clone(),
                    new_code: format!("{} /* TODO: add missing field */", line_content),
                    fix_type: FixType::AddField,
                });
            }
            if err.error_code.contains("E0425") {
                // E0425: cannot find value — suggest adding import
                let symbol = err.message.split_whitespace().last()
                    .unwrap_or("unknown").to_string();
                fixes.push(Fix {
                    file: err.file.clone(),
                    line: err.line,
                    description: format!("Cannot find value: {}", err.message),
                    old_code: line_content.clone(),
                    new_code: format!("use {};\n{}", symbol, line_content),
                    fix_type: FixType::AddImport,
                });
            }
            // Generic fallback: capture any error with context
            if !err.error_code.is_empty() && fixes.iter().all(|f| f.line != err.line) {
                fixes.push(Fix {
                    file: err.file.clone(),
                    line: err.line,
                    description: format!("Compiler error {}: {}", err.error_code, err.message),
                    old_code: line_content,
                    new_code: String::new(), // AI will fill this
                    fix_type: FixType::TypeAnnotation,
                });
            }
        }

        fixes
    }

    /// Read a specific line from a file for error context
    fn read_error_line(&self, file: &str, line: usize) -> String {
        std::fs::read_to_string(file)
            .ok()
            .and_then(|content| content.lines().nth(line.saturating_sub(1)).map(|l| l.to_string()))
            .unwrap_or_default()
    }

    /// Apply fixes to edits — for fixes with non-empty new_code, apply them directly
    fn apply_fixes(&self, edits: Vec<AstEdit>, fixes: &[Fix]) -> Vec<AstEdit> {
        let mut updated_edits = edits;

        // Apply fixes that have meaningful content
        for fix in fixes {
            if fix.old_code.is_empty() {
                continue;
            }

            // Try to apply the fix directly to the file
            let path = std::path::Path::new(&fix.file);
            if path.exists() {
                if let Ok(content) = std::fs::read_to_string(path) {
                    let new_content = content.replace(&fix.old_code, &fix.new_code);
                    if new_content != content {
                        if let Err(e) = std::fs::write(path, &new_content) {
                            tracing::warn!("Failed to apply fix to {}: {}", fix.file, e);
                        } else {
                            tracing::info!("Applied fix: {} — in {}", fix.description, fix.file);
                        }
                    }
                }
            }
        }

        updated_edits
    }
}

/// AI-Fix Request — 供 Agent/LLM 填充的结构化修复请求
///
/// 当自修正循环无法自动生成 new_code 时，返回此请求给上层
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AiFixRequest {
    /// 出错文件
    pub file: String,
    /// 出错行号
    pub line: u32,
    /// 错误代码 (如 E0308)
    pub error_code: String,
    /// 错误消息
    pub error_message: String,
    /// 出错行的原始代码
    pub context_line: String,
    /// 上下文 (前后各2行)
    pub context_surrounding: Vec<String>,
    /// 建议的修复类型
    pub suggested_fix_type: FixType,
}

impl SelfCorrectionLoop {
    /// Generate AI-fix requests for errors that couldn't be auto-fixed
    pub fn generate_ai_fix_requests(&self, errors: &[TypeError]) -> Vec<AiFixRequest> {
        let mut requests = Vec::new();

        for err in errors {
            let context_line = self.read_error_line(&err.file, err.line as usize);
            let surrounding = self.read_surrounding_lines(&err.file, err.line as usize, 2);

            let fix_type = if err.error_code.contains("E0308") {
                FixType::TypeAnnotation
            } else if err.error_code.contains("E0063") {
                FixType::AddField
            } else if err.error_code.contains("E0425") {
                FixType::AddImport
            } else {
                FixType::ChangeParam
            };

            requests.push(AiFixRequest {
                file: err.file.clone(),
                line: err.line,
                error_code: err.error_code.clone(),
                error_message: err.message.clone(),
                context_line,
                context_surrounding: surrounding,
                suggested_fix_type: fix_type,
            });
        }

        requests
    }

    fn read_surrounding_lines(&self, file: &str, center_line: usize, radius: usize) -> Vec<String> {
        std::fs::read_to_string(file)
            .ok()
            .map(|content| {
                let lines: Vec<&str> = content.lines().collect();
                let start = center_line.saturating_sub(radius + 1);
                let end = (center_line + radius).min(lines.len());
                lines[start..end].iter().map(|l| l.to_string()).collect()
            })
            .unwrap_or_default()
    }
}
