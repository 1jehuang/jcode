use crate::ast::{AstAdapter, AstEdit, LanguageKind};
use crate::dependency::DependencyGraph;
use futures::future::join_all;
use std::path::Path;
use std::sync::Arc;

pub struct CrossFileProcessor<A: AstAdapter> {
    ast_adapter: Arc<A>,
}

impl<A: AstAdapter> CrossFileProcessor<A> {
    pub fn new(ast_adapter: Arc<A>) -> Self { Self { ast_adapter } }

    pub async fn process_edits(
        &self,
        edits: Vec<AstEdit>,
        deps: &DependencyGraph,
    ) -> anyhow::Result<Vec<AstEdit>> {
        let mut expanded = edits;
        let mut added = true;
        while added {
            added = false;
            let current = expanded.clone();
            for edit in &current {
                let affected = deps.affected_files(&edit.file_path);
                for file in affected {
                    if !expanded.iter().any(|e| e.file_path == file) {
                        let file_clone = file.clone();
                        expanded.push(AstEdit {
                            file_path: file_clone,
                            language: LanguageKind::from_path(Path::new(&file)),
                            operations: vec![],
                        });
                        added = true;
                    }
                }
            }
        }

        let futures: Vec<_> = expanded.iter().map(|edit| {
            let file_path = edit.file_path.clone();
            async move {
                if Path::new(&file_path).exists() {
                    tokio::fs::read_to_string(&file_path).await.ok()
                } else {
                    None
                }
            }
        }).collect();

        let _ = join_all(futures).await;
        Ok(expanded)
    }
}