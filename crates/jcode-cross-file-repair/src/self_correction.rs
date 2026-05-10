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
    pub old_code: String,    // 原文 (用于搜索替换)
    pub new_code: String,    // 替换后 (应用修复)
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
        tracing::info!("✅ Applied fix: {} — replaced in {}", self.description, self.file);
        Ok(())
    }

    /// 批量应用多个 Fix
    pub async fn apply_all(fixes: &[Fix]) -> anyhow::Result<Vec<&Fix>> {
        let mut failed = Vec::new();
        for fix in fixes {
            if let Err(e) = fix.apply().await {
                tracing::warn!("❌ Fix failed: {} — {}", fix.description, e);
                failed.push(fix);
            }
        }
        Ok(failed)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FixType {
    TypeAnnotation,    // 添加类型标注
    AddField,          // 添加缺失字段
    RenameField,      // 重命名字段
    AddImport,        // 添加导入
    ChangeParam,      // 修改参数类型
    AddReturn,        // 添加返回值
    RemoveUnused,     // 删除未使用代码
}

/// 自修正循环 — 检测 → 修复 → 验证
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

        for round in 0..self.max_rounds {
            let errors = type_checker.check(".").await?;

            if errors.is_empty() {
                return Ok(current_edits);
            }

            // 将编译器错误转换为具体的 Fix
            let fixes = self.errors_to_fixes(&errors);

            let iter = CorrectionIteration {
                round,
                errors_found: vec![],
                type_errors: errors,
                fixes_applied: fixes.clone(),
                success: false,
            };
            iterations.push(iter);

            // 如果没有可用的 fix，停止
            if fixes.is_empty() {
                break;
            }

            // 应用修复
            current_edits = self.apply_fixes(current_edits, &fixes);
        }

        Ok(current_edits)
    }

    /// 将编译器错误转换为具体修复 (包含 old_code / new_code)
    fn errors_to_fixes(&self, errors: &[TypeError]) -> Vec<Fix> {
        let mut fixes = Vec::new();

        for err in errors {
            // E0308: mismatched types → 需要类型标注
            if err.error_code.contains("E0308") {
                fixes.push(Fix {
                    file: err.file.clone(),
                    line: err.line,
                    description: format!("Type mismatch: {}", err.message),
                    old_code: String::new(),  // 由 AI 填充
                    new_code: String::new(),  // 由 AI 填充
                    fix_type: FixType::TypeAnnotation,
                });
            }
            // E0063: missing field → 需要添加字段
            if err.error_code.contains("E0063") {
                fixes.push(Fix {
                    file: err.file.clone(),
                    line: err.line,
                    description: "Missing struct field".into(),
                    old_code: String::new(),
                    new_code: String::new(),
                    fix_type: FixType::AddField,
                });
            }
            // E0425: cannot find value → 需要导入
            if err.error_code.contains("E0425") {
                fixes.push(Fix {
                    file: err.file.clone(),
                    line: err.line,
                    description: format!("Cannot find value: {}", err.message),
                    old_code: String::new(),
                    new_code: String::new(),
                    fix_type: FixType::AddImport,
                });
            }
        }

        fixes
    }

    fn apply_fixes(&self, edits: Vec<AstEdit>, _fixes: &[Fix]) -> Vec<AstEdit> {
        // 在真实环境中: AI 根据 fix 生成修改后的代码
        // 当前简化实现: 标记 fix 位置供 LLM 处理
        edits
    }
}
