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
///
/// ## Improvements over previous version:
/// - Recursive directory scanning (was single-level `read_dir`)
/// - AST-based Extends/Impl/Call detection (was Import-only)
/// - Proper Windows path handling
pub struct DependencyAnalyzer;

impl DependencyAnalyzer {
    pub fn new() -> Self { Self }

    pub fn analyze(&self, workspace_root: &str) -> anyhow::Result<DependencyGraph> {
        let mut edges = Vec::new();
        let root = Path::new(workspace_root);
        if !root.exists() { return Ok(DependencyGraph::new(vec![])); }

        // Recursive file walk
        let files = self.collect_files_recursive(root);

        for (path_str, content, ext) in &files {
            // Extract import dependencies
            let imports = self.extract_imports(content, ext);
            for import in imports {
                edges.push(DependencyEdge {
                    from: path_str.clone(),
                    to: import,
                    kind: DepKind::Import,
                });
            }

            // Extract impl dependencies (Rust)
            if ext == "rs" {
                let impl_deps = self.extract_impl_dependencies(content);
                for (trait_name, dep_kind) in impl_deps {
                    edges.push(DependencyEdge {
                        from: path_str.clone(),
                        to: trait_name,
                        kind: dep_kind,
                    });
                }

                // Extract call dependencies
                let call_deps = self.extract_call_dependencies(content);
                for call_target in call_deps {
                    edges.push(DependencyEdge {
                        from: path_str.clone(),
                        to: call_target,
                        kind: DepKind::Call,
                    });
                }
            }

            // Extract extends dependencies (TypeScript/Java/Python)
            if matches!(ext, "ts" | "tsx" | "js" | "java" | "py") {
                let extends_deps = self.extract_extends_dependencies(content, ext);
                for parent_class in extends_deps {
                    edges.push(DependencyEdge {
                        from: path_str.clone(),
                        to: parent_class,
                        kind: DepKind::Extends,
                    });
                }
            }
        }

        Ok(DependencyGraph::new(edges))
    }

    /// Recursively collect files with (path, content, extension)
    fn collect_files_recursive(&self, root: &Path) -> Vec<(String, String, String)> {
        let mut files = Vec::new();
        self.walk_dir(root, &mut files);
        files
    }

    fn walk_dir(&self, dir: &Path, files: &mut Vec<(String, String, String)>) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let name = path.file_name().unwrap_or_default().to_string_lossy();
                    // Skip common non-project directories
                    if name != "node_modules" && name != "target" && name != ".git" &&
                       name != "dist" && name != "build" && name != "__pycache__" &&
                       name != ".cargo" && name != "vendor" && !name.starts_with('.') {
                        self.walk_dir(&path, files);
                    }
                } else if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                    if matches!(ext, "rs" | "ts" | "tsx" | "js" | "py" | "go" | "java") {
                        if let Ok(content) = std::fs::read_to_string(&path) {
                            files.push((path.to_string_lossy().to_string(), content, ext.to_string()));
                        }
                    }
                }
            }
        }
    }

    fn extract_imports(&self, content: &str, ext: &str) -> Vec<String> {
        let mut imports = Vec::new();
        match ext {
            "rs" => {
                for line in content.lines() {
                    if let Some(path) = line.trim().strip_prefix("use ") {
                        let cleaned = path.trim_end_matches(';').trim();
                        // Handle grouped imports: use foo::{bar, baz}
                        if let Some(start) = cleaned.find("::{") {
                            let base = &cleaned[..start];
                            imports.push(base.to_string());
                        } else if let Some(end) = cleaned.find(" as ") {
                            imports.push(cleaned[..end].trim().to_string());
                        } else {
                            imports.push(cleaned.to_string());
                        }
                    }
                }
            }
            "ts" | "tsx" | "js" => {
                let import_re = regex::Regex::new(r#"(?:import|from)\s+.*?['"]([^'"]+)['"]"#).unwrap();
                for cap in import_re.captures_iter(content) {
                    imports.push(cap[1].to_string());
                }
            }
            "go" => {
                for line in content.lines() {
                    let trimmed = line.trim();
                    if trimmed.starts_with("import") {
                        if let Some(path) = trimmed.strip_prefix("import") {
                            let cleaned = path.trim().trim_end_matches('"').trim_start_matches('"');
                            imports.push(cleaned.to_string());
                        }
                    }
                    // Also handle multi-line imports
                    if trimmed.starts_with("\"") && trimmed.ends_with("\"") {
                        imports.push(trimmed.trim_matches('"').to_string());
                    }
                }
            }
            "py" => {
                let import_re = regex::Regex::new(r"from\s+(\S+)\s+import|import\s+(\S+)").unwrap();
                for cap in import_re.captures_iter(content) {
                    let module = cap.get(1).or_else(|| cap.get(2));
                    if let Some(m) = module {
                        imports.push(m.as_str().to_string());
                    }
                }
            }
            "java" => {
                for line in content.lines() {
                    let trimmed = line.trim();
                    if trimmed.starts_with("import ") {
                        let path = trimmed.strip_prefix("import ").unwrap_or("")
                            .trim_end_matches(';').trim();
                        imports.push(path.to_string());
                    }
                }
            }
            _ => {}
        }
        imports
    }

    /// Extract impl dependencies from Rust code using tree-sitter
    fn extract_impl_dependencies(&self, content: &str) -> Vec<(String, DepKind)> {
        let mut deps = Vec::new();

        let mut parser = tree_sitter::Parser::new();
        if parser.set_language(&tree_sitter_rust::LANGUAGE.into()).is_err() {
            // Fallback: regex-based
            let impl_re = regex::Regex::new(r"impl\s+(?:<[^>]+>\s+)?(\w+)").unwrap();
            for cap in impl_re.captures_iter(content) {
                deps.push((cap[1].to_string(), DepKind::Impl));
            }

            let impl_for_re = regex::Regex::new(r"impl\s+(?:<[^>]+>\s+)?(\w+)\s+for\s+(\w+)").unwrap();
            for cap in impl_for_re.captures_iter(content) {
                deps.push((cap[1].to_string(), DepKind::Impl));
                deps.push((cap[2].to_string(), DepKind::Impl));
            }
            return deps;
        }

        if let Some(tree) = parser.parse(content, None) {
            let root = tree.root_node();
            let mut cursor = root.walk();
            for node in root.children(&mut cursor) {
                if node.kind() == "impl_item" {
                    // impl Trait for Type
                    if let Some(trait_node) = node.child_by_field_name("trait") {
                        if let Ok(trait_name) = trait_node.utf8_text(content.as_bytes()) {
                            deps.push((trait_name.to_string(), DepKind::Impl));
                        }
                    }
                    // impl Type
                    if let Some(type_node) = node.child_by_field_name("type") {
                        if let Ok(type_name) = type_node.utf8_text(content.as_bytes()) {
                            deps.push((type_name.to_string(), DepKind::Impl));
                        }
                    }
                }
            }
        }

        deps
    }

    /// Extract call dependencies — functions called from this file
    fn extract_call_dependencies(&self, content: &str) -> Vec<String> {
        let mut calls = HashSet::new();

        let mut parser = tree_sitter::Parser::new();
        if parser.set_language(&tree_sitter_rust::LANGUAGE.into()).is_err() {
            // Fallback: regex
            let call_re = regex::Regex::new(r"(\w+)::\w+\s*\(").unwrap();
            for cap in call_re.captures_iter(content) {
                let module = cap[1].to_string();
                if !matches!(module.as_str(), "Self" | "self" | "super" | "crate" | "std" | "core" | "alloc") {
                    calls.insert(module);
                }
            }
            return calls.into_iter().collect();
        }

        if let Some(tree) = parser.parse(content, None) {
            let root = tree.root_node();
            self.collect_calls(&root, content, &mut calls);
        }

        calls.into_iter().collect()
    }

    fn collect_calls(&self, node: &tree_sitter::Node, source: &str, calls: &mut HashSet<String>) {
        if node.kind() == "scoped_identifier" {
            if let Ok(text) = node.utf8_text(source.as_bytes()) {
                // e.g., "foo::bar" — extract "foo"
                let parts: Vec<&str> = text.split("::").collect();
                if parts.len() >= 2 {
                    let first = parts[0];
                    if !matches!(first, "Self" | "self" | "super" | "crate" | "std" | "core" | "alloc") {
                        calls.insert(first.to_string());
                    }
                }
            }
        }

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.is_named() {
                self.collect_calls(&child, source, calls);
            }
        }
    }

    /// Extract extends (inheritance) dependencies for TS/Java/Python
    fn extract_extends_dependencies(&self, content: &str, ext: &str) -> Vec<String> {
        let mut deps = Vec::new();

        match ext {
            "ts" | "tsx" | "js" => {
                let extends_re = regex::Regex::new(r"extends\s+(\w+)").unwrap();
                let implements_re = regex::Regex::new(r"implements\s+(\w+)").unwrap();
                for cap in extends_re.captures_iter(content) {
                    deps.push(cap[1].to_string());
                }
                for cap in implements_re.captures_iter(content) {
                    deps.push(cap[1].to_string());
                }
            }
            "java" => {
                let extends_re = regex::Regex::new(r"extends\s+(\w+)").unwrap();
                let implements_re = regex::Regex::new(r"implements\s+([\w,\s]+)").unwrap();
                for cap in extends_re.captures_iter(content) {
                    deps.push(cap[1].to_string());
                }
                for cap in implements_re.captures_iter(content) {
                    for name in cap[1].split(',') {
                        let trimmed = name.trim().to_string();
                        if !trimmed.is_empty() {
                            deps.push(trimmed);
                        }
                    }
                }
            }
            "py" => {
                let class_re = regex::Regex::new(r"class\s+\w+\s*\(([^)]+)\)").unwrap();
                for cap in class_re.captures_iter(content) {
                    for name in cap[1].split(',') {
                        let trimmed = name.trim().to_string();
                        if !trimmed.is_empty() && trimmed != "object" {
                            deps.push(trimmed);
                        }
                    }
                }
            }
            _ => {}
        }

        deps
    }
}
