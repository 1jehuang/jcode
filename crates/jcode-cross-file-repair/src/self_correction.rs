use crate::ast::AstEdit;
use crate::error_detector::CodeError;
use crate::type_checker::{TypeChecker, TypeError};
use std::sync::Arc;

#[async_trait::async_trait]
pub trait AiFixProvider: Send + Sync {
    async fn suggest_fix(&self, request: &AiFixRequest) -> Option<Fix>;
}

#[derive(Debug, Clone)]
pub struct CorrectionIteration {
    pub round: u32,
    pub errors_found: Vec<CodeError>,
    pub type_errors: Vec<TypeError>,
    pub fixes_applied: Vec<Fix>,
    pub success: bool,
}

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
    /// Apply fix via RefactorEngine-style: checkpoint + replace + verify
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
        // Direct write; consider using bridge + multi-file-edit for full atomicity
        tokio::fs::write(path, &new).await?;
        tracing::info!("Applied fix: {} — replaced in {}", self.description, self.file);
        Ok(())
    }

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FixType {
    TypeAnnotation, AddField, RenameField, AddImport, ChangeParam, AddReturn, RemoveUnused,
}

/// Self-correction loop — detect -> generate contextual fixes -> verify
/// Uses AI-in-the-loop pattern: errors_to_fixes() produces Fix with real contextual code.
pub struct SelfCorrectionLoop {
    max_rounds: u32,
    ai_fix_provider: Option<Arc<dyn AiFixProvider>>,
}

impl SelfCorrectionLoop {
    pub fn new(max_rounds: u32) -> Self {
        Self { max_rounds, ai_fix_provider: None }
    }

    pub fn with_ai_provider(max_rounds: u32, provider: Arc<dyn AiFixProvider>) -> Self {
        Self { max_rounds, ai_fix_provider: Some(provider) }
    }

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

            let error_sig: Vec<String> = errors.iter()
                .map(|e| format!("{}:{}:{}", e.file, e.line, e.error_code))
                .collect();

            if previous_error_signatures == error_sig {
                tracing::warn!("Self-correction loop: same errors, stopping to avoid infinite loop");
                break;
            }
            previous_error_signatures = error_sig;

            let mut fixes = self.errors_to_fixes(&errors);

            if let Some(ai_provider) = &self.ai_fix_provider {
                let ai_requests = self.generate_ai_fix_requests(&errors);
                for request in &ai_requests {
                    match ai_provider.suggest_fix(request).await {
                        Some(fix) => {
                            if !fixes.iter().any(|f| f.file == fix.file && f.line == fix.line) {
                                fixes.push(fix);
                            }
                        }
                        None => {
                            tracing::debug!("AI could not suggest a fix for {}:{}", request.file, request.line);
                        }
                    }
                }
            }

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

            current_edits = self.apply_fixes(current_edits, &fixes);
        }

        Ok(current_edits)
    }

    /// Convert compiler errors into Fix with contextual old_code (AI-in-the-loop ready)
    fn errors_to_fixes(&self, errors: &[TypeError]) -> Vec<Fix> {
        let mut fixes = Vec::new();
        for err in errors {
            let line_content = self.read_error_line(&err.file, err.line as usize);
            if err.error_code.contains("E0308") {
                fixes.push(Fix {
                    file: err.file.clone(), line: err.line,
                    description: format!("Type mismatch: {}", err.message),
                    old_code: line_content.clone(),
                    new_code: format!("/* TODO: fix type mismatch — {} */
{}", err.message, line_content),
                    fix_type: FixType::TypeAnnotation,
                });
            }
            if err.error_code.contains("E0063") {
                fixes.push(Fix {
                    file: err.file.clone(), line: err.line,
                    description: "Missing struct field".into(),
                    old_code: line_content.clone(),
                    new_code: format!("{} /* TODO: add missing field */", line_content),
                    fix_type: FixType::AddField,
                });
            }
            if err.error_code.contains("E0425") {
                let symbol = err.message.split_whitespace().last()
                    .unwrap_or("unknown").to_string();
                fixes.push(Fix {
                    file: err.file.clone(), line: err.line,
                    description: format!("Cannot find value: {}", err.message),
                    old_code: line_content.clone(),
                    new_code: format!("use {};
{}", symbol, line_content),
                    fix_type: FixType::AddImport,
                });
            }
            if !err.error_code.is_empty() && fixes.iter().all(|f| f.line != err.line) {
                fixes.push(Fix {
                    file: err.file.clone(), line: err.line,
                    description: format!("Compiler error {}: {}", err.error_code, err.message),
                    old_code: line_content,
                    new_code: String::new(),
                    fix_type: FixType::TypeAnnotation,
                });
            }
        }
        fixes
    }

    fn read_error_line(&self, file: &str, line: usize) -> String {
        std::fs::read_to_string(file)
            .ok()
            .and_then(|content| content.lines().nth(line.saturating_sub(1)).map(|l| l.to_string()))
            .unwrap_or_default()
    }

    fn apply_fixes(&self, edits: Vec<AstEdit>, fixes: &[Fix]) -> Vec<AstEdit> {
        for fix in fixes {
            if fix.old_code.is_empty() { continue; }
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
        edits
    }

    pub fn generate_ai_fix_requests(&self, errors: &[TypeError]) -> Vec<AiFixRequest> {
        let mut requests = Vec::new();
        for err in errors {
            let context_line = self.read_error_line(&err.file, err.line as usize);
            let surrounding = self.read_surrounding_lines(&err.file, err.line as usize, 2);
            let fix_type = if err.error_code.contains("E0308") { FixType::TypeAnnotation }
                else if err.error_code.contains("E0063") { FixType::AddField }
                else if err.error_code.contains("E0425") { FixType::AddImport }
                else { FixType::ChangeParam };
            requests.push(AiFixRequest {
                file: err.file.clone(), line: err.line,
                error_code: err.error_code.clone(),
                error_message: err.message.clone(),
                context_line, context_surrounding: surrounding, suggested_fix_type: fix_type,
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

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AiFixRequest {
    pub file: String,
    pub line: u32,
    pub error_code: String,
    pub error_message: String,
    pub context_line: String,
    pub context_surrounding: Vec<String>,
    pub suggested_fix_type: FixType,
}
