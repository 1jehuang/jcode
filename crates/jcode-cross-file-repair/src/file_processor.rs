use crate::ast::{AstAdapter, AstEdit, LanguageKind};
use crate::bridge::EditBridge;
use crate::dependency::DependencyGraph;
use jcode_multi_file_edit::MultiFileEngine;
use std::path::Path;
use std::sync::Arc;

pub struct CrossFileProcessor<A: AstAdapter> {
    #[allow(dead_code)]
    ast_adapter: Arc<A>,
}

impl<A: AstAdapter> CrossFileProcessor<A> {
    pub fn new(ast_adapter: Arc<A>) -> Self { Self { ast_adapter } }

    pub async fn process_edits(
        &self,
        edits: Vec<AstEdit>,
        deps: &DependencyGraph,
    ) -> anyhow::Result<Vec<AstEdit>> {
        // Step 1: Expand edits to include affected files
        let mut expanded = edits;
        let mut added = true;
        while added {
            added = false;
            let current = expanded.clone();
            for edit in &current {
                let affected = deps.affected_files(&edit.file_path);
                for file in affected {
                    let file_check = file.clone();
                    if !expanded.iter().any(|e| e.file_path == file_check) {
                        let file_path = file.clone();
                        expanded.push(AstEdit {
                            file_path,
                            language: LanguageKind::from_path(Path::new(&file)),
                            operations: vec![],
                        });
                        added = true;
                    }
                }
            }
        }

        // Step 2: Convert AstEdit -> FileSet via EditBridge, then apply via MultiFileEngine
        let ts_adapter = crate::ast::TreeSitterAstAdapter::default();
        let bridge = EditBridge::new(ts_adapter);
        let file_set = bridge.convert(expanded.clone()).await?;
        let multi_engine = MultiFileEngine::new();
        let commit_result = multi_engine.execute_atomic(vec![file_set]).await?;

        if !commit_result.success {
            anyhow::bail!("Atomic edit failed: {}", commit_result.error.unwrap_or_default());
        }

        Ok(expanded)
    }
}
