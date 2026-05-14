//! # jcode-cross-file-repair
//! Cross-file repair engine with type-checking self-correction loop.
//!
//! ## Architecture
//!
//! ```text
//! AI Repair Suggestion
//!       ↓
//! DependencyAnalyzer  ──→  identifies all affected files
//!       ↓
//! ASTAdapter (per language)  ──→ parse → edit → validate
//!       ↓
//! ParallelFileProcessor  ──→ tokio::join! on all files
//!       ↓
//! TypeChecker (rustc bridge)  ──→ compile check
//!       ↓
//! SelfCorrectionLoop  ──→ if errors: re-prompt AI with errors
//!       ↓
//! Final validated changes
//! ```

mod ast;
mod dependency;
mod type_checker;
mod self_correction;
mod file_processor;
mod error_detector;
pub mod bridge;

pub use ast::{AstAdapter, AstNode, LanguageKind, AstEdit, AstEditOp, TreeSitterAstAdapter};
pub use dependency::{DependencyAnalyzer, DependencyGraph, DependencyEdge, DepKind};
pub use type_checker::TypeChecker;
pub use self_correction::{SelfCorrectionLoop, CorrectionIteration, Fix, FixType, AiFixRequest};
pub use file_processor::CrossFileProcessor;
pub use error_detector::{ErrorDetector, CodeError, MismatchDirection};
pub use bridge::EditBridge;

use std::sync::Arc;

pub struct CrossFileRepairEngine<A: AstAdapter> {
    dep_analyzer: DependencyAnalyzer,
    ast_adapter: Arc<A>,
    type_checker: TypeChecker,
    correction_loop: SelfCorrectionLoop,
}

impl<A: AstAdapter> CrossFileRepairEngine<A> {
    pub fn new(ast_adapter: Arc<A>, type_checker: TypeChecker) -> Self {
        Self {
            dep_analyzer: DependencyAnalyzer::new(),
            ast_adapter,
            type_checker,
            correction_loop: SelfCorrectionLoop::new(3),
        }
    }

    pub async fn validate_and_repair(
        &self,
        edits: Vec<AstEdit>,
        workspace_root: &str,
    ) -> anyhow::Result<Vec<AstEdit>> {
        let deps = self.dep_analyzer.analyze(workspace_root)?;

        let processor = CrossFileProcessor::new(self.ast_adapter.clone());
        let processed = processor.process_edits(edits, &deps).await?;

        let final_edits = self.correction_loop
            .run(processed, &self.type_checker)
            .await?;

        Ok(final_edits)
    }
}