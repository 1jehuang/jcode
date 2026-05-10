use std::collections::{HashMap, HashSet};
use std::path::Path;

/// A dependency edge between two files.
#[derive(Debug, Clone)]
pub struct DependencyEdge {
    pub from: String,
    pub to: String,
    pub kind: DepKind,
}

/// Type of dependency relationship.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepKind {
    Import,    // A imports B
    Extends,   // A extends B (class inheritance)
    Impl,      // A implements B (trait/interface)
    Call,      // A calls function from B
}

/// The full dependency graph of a project.
#[derive(Debug, Clone)]
pub struct DependencyGraph {
    pub edges: Vec<DependencyEdge>,
    pub reverse_map: HashMap<String, Vec<String>>,
    pub forward_map: HashMap<String, Vec<String>>,
}

impl DependencyGraph {
    pub fn new(edges: Vec<DependencyEdge>) -> Self {
        let mut reverse: HashMap<String, Vec<String>> = HashMap::new();
        let mut forward: HashMap<String, Vec<String>> = HashMap::new();
        for e in &edges {
            forward.entry(e.from.clone()).or_default().push(e.to.clone());
            reverse.entry(e.to.clone()).or_default().push(e.from.clone());
        }
        Self { edges, reverse_map: reverse, forward_map: forward }
    }

    /// Find all files transitively affected by changes to `changed_file`.
    pub fn affected_files(&self, changed_file: &str) -> HashSet<String> {
        let mut affected = HashSet::new();
        let mut stack = vec![changed_file.to_string()];
        while let Some(file) = stack.pop() {
            if let Some(dependents) = self.reverse_map.get(&file) {
                for dep in dependents {
                    if affected.insert(dep.clone()) {
                        stack.push(dep.clone());
                    }
                }
            }
        }
        affected
    }
}

/// Scans a workspace for import dependencies.
pub struct DependencyAnalyzer;

impl DependencyAnalyzer {
    pub fn new() -> Self { Self }

    pub fn analyze(&self, workspace_root: &str) -> anyhow::Result<DependencyGraph> {
        let mut edges = Vec::new();
        let root = Path::new(workspace_root);
        if !root.exists() { return Ok(DependencyGraph::new(vec![])); }

        // Walk files and extract imports
        if let Ok(entries) = std::fs::read_dir(root) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() { continue; }
                if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                    if matches!(ext, "rs" | "ts" | "tsx" | "js" | "py" | "go" | "java") {
                        if let Ok(content) = std::fs::read_to_string(&path) {
                            let imports = self.extract_imports(&content, ext);
                            let from = path.to_string_lossy().to_string();
                            for import in imports {
                                edges.push(DependencyEdge {
                                    from: from.clone(),
                                    to: import,
                                    kind: DepKind::Import,
                                });
                            }
                        }
                    }
                }
            }
        }
        Ok(DependencyGraph::new(edges))
    }

    fn extract_imports(&self, content: &str, ext: &str) -> Vec<String> {
        let mut imports = Vec::new();
        match ext {
            "rs" => {
                for line in content.lines() {
                    if let Some(path) = line.strip_prefix("use ") {
                        if let Some(end) = path.find(" as ") {
                            imports.push(path[..end].trim().to_string());
                        } else if let Some(end) = path.find("::") {
                            imports.push(path[..end].trim().to_string());
                        } else {
                            imports.push(path.trim_end_matches(';').to_string());
                        }
                    }
                }
            }
            "ts" | "tsx" | "js" => {
                for line in content.lines() {
                    let trimmed = line.trim();
                    if trimmed.starts_with("import ") || trimmed.starts_with("from ") {
                        if let Some(start) = trimmed.find('\'') {
                            if let Some(end) = trimmed[start+1..].find('\'') {
                                imports.push(trimmed[start+1..start+1+end].to_string());
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        imports
    }
}
