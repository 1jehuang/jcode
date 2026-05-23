//! 知识图谱 → 语义重构 → 编译引擎 闭环
//!
//! 理解代码 → 精确修改 → 验证 全自动流水线
//!
//! 闭环: 知识图谱分析 → AST语义重构 → 编译验证 → 结果报告
//!
//! 打通: src/knowledge_agents/ (7-Agent) 
//!       src/refactor/semantic.rs (AstRenamer)
//!       src/compilation_engine.rs (CompilationEngine + FixEngine)

use std::path::{Path, PathBuf};

use crate::knowledge_agents::{
    PipelineConfig, project_scanner, file_analyzer,
};
use crate::refactor::semantic::AstRenamer;
use crate::compilation_engine::{CompilationEngine, FixEngine};

/// 重构流水线结果
#[derive(Debug, Clone)]
pub struct RefactorPipelineResult {
    pub files_analyzed: usize,
    pub refactorings_applied: Vec<String>,
    pub compile_passed: bool,
    pub errors_remaining: usize,
    pub fixes_applied: Vec<String>,
}

/// 知识图谱 → 语义重构 → 编译引擎 闭环
pub struct RefactorVerifyPipeline {
    workspace: PathBuf,
}

impl RefactorVerifyPipeline {
    pub fn new(workspace: &Path) -> Self {
        Self { workspace: workspace.to_path_buf() }
    }

    /// 执行完整闭环: 分析 → 重构 → 验证
    pub async fn run(&self) -> Result<RefactorPipelineResult, String> {
        // Phase 1: 知识图谱分析 (理解代码)
        println!("[Phase 1] 知识图谱分析...");
        let analysis = self.analyze_codebase().await?;

        // Phase 2: 语义重构 (精确修改)
        println!("[Phase 2] 语义重构...");
        let refactorings = self.apply_refactorings(&analysis).await?;

        // Phase 3: 编译验证 (验证修改)
        println!("[Phase 3] 编译验证...");
        let (compile_passed, errors_remaining, fixes) = self.verify_and_fix().await?;

        Ok(RefactorPipelineResult {
            files_analyzed: analysis.len(),
            refactorings_applied: refactorings,
            compile_passed,
            errors_remaining,
            fixes_applied: fixes,
        })
    }

    /// Phase 1: 知识图谱分析
    /// 运行 7-Agent 流水线 → 输出文件结构 + 符号 + 架构分层 + 业务域
    async fn analyze_codebase(&self) -> Result<Vec<AnalysisResult>, String> {
        let mut results = Vec::new();

        // Agent 1: 扫描文件
        let config = PipelineConfig::default();
        let files = project_scanner::scan_project(&self.workspace, &config).await?;

        // Agent 2: 分析符号 + 依赖
        let file_paths: Vec<String> = files.iter().map(|f| f.path.clone()).collect();
        let analyses = file_analyzer::analyze_files(&self.workspace, &file_paths, 5).await?;

        for a in &analyses {
            // 提取重构机会
            let refactor_hints = self.detect_refactor_opportunities(&a.file_path).await;

            results.push(AnalysisResult {
                file: a.file_path.clone(),
                language: a.language.clone(),
                symbols: a.symbols.iter().map(|s| s.name.clone()).collect(),
                dependencies: a.dependencies.clone(),
                complexity: format!("{:?}", a.complexity),
                refactor_hints,
            });
        }

        Ok(results)
    }

    /// Phase 2: 语义重构
    /// 根据分析结果执行精确的 AST 重命名/提取/移动
    async fn apply_refactorings(&self, analysis: &[AnalysisResult]) -> Result<Vec<String>, String> {
        let renamer = AstRenamer::new(&self.workspace);
        let mut applied = Vec::new();

        for file_info in analysis {
            // 为每个文件检测具体的重构机会
            let path = self.workspace.join(&file_info.file);
            let content = tokio::fs::read_to_string(&path).await.unwrap_or_default();
            let lines: Vec<&str> = content.lines().collect();

            // 检测过长符号名 → 建议重命名
            for sym in &file_info.symbols {
                if sym.len() > 40 {
                    let short_name = &sym[..30];
                    match renamer.rename_ast(&file_info.file, sym, short_name).await {
                        Ok(msg) => applied.push(format!("Renamed '{}' → '{}': {}", sym, short_name, msg)),
                        Err(_) => {}
                    }
                }
            }

            // 检测过长函数 (超过100行) → 标记
            let mut in_fn = false;
            let mut fn_lines = 0usize;
            for line in &lines {
                let t = line.trim();
                if t.starts_with("fn ") || t.starts_with("pub fn ") { in_fn = true; fn_lines = 0; }
                if in_fn { fn_lines += 1; }
                if in_fn && t == "}" && fn_lines > 100 {
                    applied.push(format!("Long function ({} lines) in {}", fn_lines, file_info.file));
                    in_fn = false;
                }
            }
        }

        Ok(applied)
    }

    /// Phase 3: 编译验证 + 自动修复
    async fn verify_and_fix(&self) -> Result<(bool, usize, Vec<String>), String> {
        let engine = CompilationEngine::new(&self.workspace);
        let fix_engine = FixEngine::new(&self.workspace);

        let result = engine.cargo_check(&[]).await;
        if result.success {
            return Ok((true, 0, vec!["✅ Compilation passed".to_string()]));
        }

        // 尝试修复
        let fixes = fix_engine.fix_errors(&result.errors, &result.raw_output).await?;
        let recheck = engine.cargo_check(&[]).await;

        let fix_msgs: Vec<String> = fixes.iter().map(|f| format!("Fixed: {}", f.file)).collect();
        Ok((recheck.success, recheck.errors.len(), fix_msgs))
    }

    /// 检测重构机会
    async fn detect_refactor_opportunities(&self, file: &str) -> Vec<String> {
        let mut hints = Vec::new();
        let path = self.workspace.join(file);
        let content = tokio::fs::read_to_string(&path).await.unwrap_or_default();

        // 检测未使用的导入
        for line in content.lines() {
            let t = line.trim();
            if t.starts_with("use ") && t.ends_with(';') {
                let import = t.trim_start_matches("use ").trim_end_matches(';');
                let name = import.split("::").last().unwrap_or("");
                if !content.lines().skip(1).any(|l| l.contains(name)) {
                    hints.push(format!("Unused import: {}", t));
                }
            }
        }

        // 检测 TODO/FIXME
        for (i, line) in content.lines().enumerate() {
            if line.contains("TODO") || line.contains("FIXME") {
                hints.push(format!("Line {}: {}", i + 1, line.trim()));
            }
        }

        // 检测硬编码字符串 (3次以上重复)
        let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for line in content.lines() {
            for word in line.split_whitespace() {
                if word.starts_with('"') && word.len() > 10 {
                    *counts.entry(word).or_insert(0) += 1;
                }
            }
        }
        for (s, c) in &counts {
            if *c >= 3 {
                hints.push(format!("Repeated string '{}...' ({} times) → extract as const", &s[..15.min(s.len())], c));
            }
        }

        hints
    }
}

/// 分析结果
#[derive(Debug, Clone)]
pub struct AnalysisResult {
    pub file: String,
    pub language: String,
    pub symbols: Vec<String>,
    pub dependencies: Vec<String>,
    pub complexity: String,
    pub refactor_hints: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_refactor_opportunities() {
        let temp = std::env::temp_dir().join("carpai-pipeline-test");
        let _ = std::fs::create_dir_all(&temp.join("src"));
        std::fs::write(temp.join("src/main.rs"),
            "use std::collections::HashMap;\nfn main() {\n    let x = 1;\n    let y = \"hello world test\";\n    let z = \"hello world test\";\n    let w = \"hello world test\";\n    // TODO: implement\n}\n").ok();

        let pipeline = RefactorVerifyPipeline::new(&temp);
        let hints = pipeline.detect_refactor_opportunities("src/main.rs").await;
        assert!(!hints.is_empty(), "Should detect issues");
        assert!(hints.iter().any(|h| h.contains("TODO")), "Should detect TODO");
        assert!(hints.iter().any(|h| h.contains("RefactorVerifyPipeline")), "Should detect repeated string");

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[tokio::test]
    async fn test_pipeline_end_to_end() {
        let temp = std::env::temp_dir().join("carpai-pipeline-e2e");
        let _ = std::fs::create_dir_all(&temp.join("src"));
        std::fs::write(temp.join("Cargo.toml"),
            "[package]\nname = \"test\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[dependencies]\n").ok();
        std::fs::write(temp.join("src/main.rs"), "fn main() { println!(\"hello\"); }\n").ok();

        let pipeline = RefactorVerifyPipeline::new(&temp);
        let result = pipeline.run().await.unwrap();
        assert!(result.files_analyzed > 0);
        // Compilation may or may not pass depending on rustc availability

        let _ = std::fs::remove_dir_all(&temp);
    }
}
