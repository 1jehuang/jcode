//! 文件依赖分析引擎
//!
//! 分析项目中的文件依赖关系（import/use/mod/require），
//! 用于评估跨文件变更的影响范围。

use anyhow::Result;
use regex::Regex;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;

/// 文件依赖信息
#[derive(Debug, Clone)]
pub struct FileDependency {
    pub path: String,
    /// 此文件导入的文件列表
    pub imports: Vec<String>,
    /// 导入此文件的文件列表
    pub imported_by: Vec<String>,
    /// 是否为入口点 (main.rs, lib.rs, index.js, __init__.py)
    pub is_entry_point: bool,
}

/// 变更影响
#[derive(Debug, Clone)]
pub struct ChangeImpact {
    pub file: String,
    pub impact_type: ImpactType,
    /// 受此变更下游影响的文件
    pub downstream_files: Vec<String>,
    pub risk: crate::planner::plan::ImpactLevel,
}

/// 影响类型
#[derive(Debug, Clone, PartialEq)]
pub enum ImpactType {
    /// 文件被直接修改
    Direct,
    /// 文件依赖被修改的文件
    Transitive,
    /// 文件定义了被其他文件使用的接口
    Interface,
}

/// 依赖分析器
pub struct DependencyAnalyzer {
    workspace_root: std::path::PathBuf,
    parser_cache: Arc<RwLock<HashMap<String, Vec<String>>>>,
}

impl DependencyAnalyzer {
    pub fn new(workspace_root: &std::path::Path) -> Self {
        Self {
            workspace_root: workspace_root.to_path_buf(),
            parser_cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// 分析单个文件的依赖
    pub async fn analyze_file(&self, path: &str) -> Result<FileDependency> {
        let full_path = self.workspace_root.join(path);
        let content = tokio::fs::read_to_string(&full_path).await?;
        let imports = self.parse_imports(path, &content);

        // 缓存解析结果
        {
            let mut cache = self.parser_cache.write().await;
            cache.insert(path.to_string(), imports.clone());
        }

        let is_entry = is_entry_point(path);

        Ok(FileDependency {
            path: path.to_string(),
            imports,
            imported_by: Vec::new(), // filled by build_dependency_graph
            is_entry_point: is_entry,
        })
    }

    /// 查找受变更影响的文件
    pub async fn find_impacted_files(&self, changed_files: &[String]) -> Result<Vec<ChangeImpact>> {
        let graph = self.build_dependency_graph().await?;
        let mut impacts = Vec::new();

        for changed in changed_files {
            // Direct impact
            impacts.push(ChangeImpact {
                file: changed.clone(),
                impact_type: ImpactType::Direct,
                downstream_files: Vec::new(),
                risk: crate::planner::plan::ImpactLevel::Low,
            });

            // Find transitive impacts via BFS
            let mut visited = HashSet::new();
            let mut queue = VecDeque::new();
            queue.push_back(changed.clone());
            visited.insert(changed.clone());

            let mut downstream = Vec::new();

            while let Some(current) = queue.pop_front() {
                if let Some(dependents) = graph.get(&current) {
                    for dependent in dependents {
                        if visited.insert(dependent.clone()) {
                            downstream.push(dependent.clone());
                            queue.push_back(dependent.clone());
                        }
                    }
                }
            }

            if !downstream.is_empty() {
                let risk = if downstream.len() > 10 {
                    crate::planner::plan::ImpactLevel::Critical
                } else if downstream.len() > 3 {
                    crate::planner::plan::ImpactLevel::High
                } else {
                    crate::planner::plan::ImpactLevel::Medium
                };

                impacts.push(ChangeImpact {
                    file: changed.clone(),
                    impact_type: ImpactType::Transitive,
                    downstream_files: downstream,
                    risk,
                });
            }
        }

        Ok(impacts)
    }

    /// 构建完整的依赖图 (file -> files that depend on it)
    pub async fn build_dependency_graph(&self) -> Result<HashMap<String, Vec<String>>> {
        let mut graph: HashMap<String, Vec<String>> = HashMap::new();

        // Collect all source files
        let files = self.collect_source_files().await?;

        // Parse each file's imports and build reverse dependency graph
        for file in &files {
            let deps = match self.analyze_file(file).await {
                Ok(dep) => dep.imports,
                Err(_) => continue, // Skip unreadable files
            };

            for dep_path in &deps {
                // Normalize the import path to a file path
                if let Some(resolved) = self.resolve_import(file, dep_path) {
                    graph.entry(resolved).or_default().push(file.clone());
                }
            }
        }

        Ok(graph)
    }

    /// 检测循环依赖
    pub async fn detect_circular(&self, file: &str) -> Result<Vec<Vec<String>>> {
        let graph = self.build_dependency_graph().await?;
        let reverse_graph: HashMap<String, Vec<String>> = self.build_reverse_graph().await?;
        let mut cycles = Vec::new();

        // Find all paths from file back to itself
        let mut path = Vec::new();
        let mut visited = HashSet::new();
        self.find_cycles_dfs(file, &reverse_graph, &mut visited, &mut path, &mut cycles);

        Ok(cycles)
    }

    /// 解析文件导入语句
    fn parse_imports(&self, file_path: &str, content: &str) -> Vec<String> {
        let ext = Path::new(file_path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();

        match ext.as_str() {
            "rs" => self.parse_rust_imports(content),
            "py" => self.parse_python_imports(content),
            "ts" | "tsx" | "js" | "jsx" => self.parse_typescript_imports(content),
            "go" => self.parse_go_imports(content),
            _ => Vec::new(),
        }
    }

    fn parse_rust_imports(&self, content: &str) -> Vec<String> {
        let mut imports = Vec::new();
        // use std::collections::HashMap;
        // use crate::module::submodule;
        // mod module_name;
        let re = Regex::new(r#"(?m)^\s*(?:use|mod)\s+([a-zA-Z0-9_:*{}]+)\s*;"#).unwrap();
        for cap in re.captures_iter(content) {
            let import_path = cap[1].to_string();
            // Extract base module path
            if let Some(base) = import_path.split("::").next() {
                imports.push(format!("{}.rs", base.replace("crate", "src")));
            }
        }
        imports
    }

    fn parse_python_imports(&self, content: &str) -> Vec<String> {
        let mut imports = Vec::new();
        // import os
        // from pathlib import Path
        // from .module import something
        let re = Regex::new(r#"(?m)^\s*(?:import|from)\s+([a-zA-Z0-9_.]+)"#).unwrap();
        for cap in re.captures_iter(content) {
            let module_path = cap[1].to_string();
            // Convert Python module path to file path
            let file_path = format!("{}.py", module_path.replace(".", "/"));
            // Only include local imports (relative or same project)
            if !module_path.starts_with('_') && !module_path.starts_with("builtins") {
                imports.push(file_path);
            }
        }
        imports
    }

    fn parse_typescript_imports(&self, content: &str) -> Vec<String> {
        let mut imports = Vec::new();
        // import { X } from './module'
        // import X from 'module'
        // const X = require('./module')
        let re = Regex::new(
            r#"(?m)(?:from|require)\s*['"]([^'"]+)['"]"#,
        ).unwrap();
        for cap in re.captures_iter(content) {
            let module_path = &cap[1];
            // Only local imports (starting with ./ or ../)
            if module_path.starts_with("./") || module_path.starts_with("../") {
                let file_path = resolve_relative_path(module_path);
                imports.push(file_path);
            }
        }
        imports
    }

    fn parse_go_imports(&self, content: &str) -> Vec<String> {
        let mut imports = Vec::new();
        // import "module/path"
        // import "module/path/sub"
        let re = Regex::new(r#"(?m)["']([a-zA-Z0-9_/.-]+)["']"#).unwrap();
        for cap in re.captures_iter(content) {
            let module_path = &cap[1];
            // Only local project imports (check if it looks like internal path)
            if !module_path.contains('.') {
                imports.push(format!("{}.go", module_path));
            }
        }
        imports
    }

    /// 解析导入路径为文件路径
    fn resolve_import(&self, _from_file: &str, import_path: &str) -> Option<String> {
        let normalized = import_path
            .trim_start_matches("crate::")
            .replace("::", "/");

        // Try common extensions
        let exts = ["", ".rs", ".py", ".ts", ".tsx", ".js", ".jsx", ".go"];
        for ext in &exts {
            let candidate = format!("{}{}", normalized, ext);
            let full_path = self.workspace_root.join(&candidate);
            if full_path.exists() {
                return Some(candidate);
            }
        }

        // Try /mod.rs, /__init__.py for directories
        for dir_ext in &["/mod.rs", "/__init__.py", "/index.ts", "/index.js"] {
            let candidate = format!("{}{}", normalized, dir_ext);
            let full_path = self.workspace_root.join(&candidate);
            if full_path.exists() {
                return Some(candidate);
            }
        }

        None
    }

    /// 构建反向依赖图 (file -> files it imports)
    async fn build_reverse_graph(&self) -> Result<HashMap<String, Vec<String>>> {
        let files = self.collect_source_files().await?;
        let mut reverse: HashMap<String, Vec<String>> = HashMap::new();

        for file in &files {
            let deps = self.parse_imports_from_cache(file).await;
            reverse.insert(file.clone(), deps);
        }

        Ok(reverse)
    }

    async fn parse_imports_from_cache(&self, file: &str) -> Vec<String> {
        let cache = self.parser_cache.read().await;
        if let Some(imports) = cache.get(file) {
            return imports.clone();
        }
        drop(cache);

        // Parse and cache
        let full_path = self.workspace_root.join(file);
        if let Ok(content) = tokio::fs::read_to_string(&full_path).await {
            let imports = self.parse_imports(file, &content);
            let mut cache = self.parser_cache.write().await;
            cache.insert(file.to_string(), imports.clone());
            return imports;
        }

        Vec::new()
    }

    /// 收集所有源文件
    async fn collect_source_files(&self) -> Result<Vec<String>> {
        let mut files = Vec::new();

        if !self.workspace_root.exists() {
            return Ok(files);
        }

        let extensions = ["rs", "py", "ts", "tsx", "js", "jsx", "go"];
        let mut dirs_to_visit = vec![self.workspace_root.clone()];

        while let Some(dir) = dirs_to_visit.pop() {
            let mut entries = tokio::fs::read_dir(&dir).await?;
            while let Some(entry) = entries.next_entry().await? {
                let path = entry.path();
                if path.is_dir() {
                    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    // Skip hidden dirs, node_modules, target, .git
                    if !name.starts_with('.') && name != "node_modules" && name != "target"
                        && name != "__pycache__" && name != ".venv"
                    {
                        dirs_to_visit.push(path);
                    }
                } else if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                    if extensions.contains(&ext) {
                        if let Ok(rel) = path.strip_prefix(&self.workspace_root) {
                            files.push(rel.to_string_lossy().to_string());
                        }
                    }
                }
            }
        }

        Ok(files)
    }

    fn find_cycles_dfs(
        &self,
        current: &str,
        graph: &HashMap<String, Vec<String>>,
        visited: &mut HashSet<String>,
        path: &mut Vec<String>,
        cycles: &mut Vec<Vec<String>>,
    ) {
        if path.contains(&current.to_string()) {
            let cycle_start = path.iter().position(|p| p == current).unwrap();
            let cycle = path[cycle_start..].to_vec();
            cycles.push(cycle);
            return;
        }

        if visited.contains(current) {
            return;
        }

        visited.insert(current.to_string());
        path.push(current.to_string());

        if let Some(deps) = graph.get(current) {
            for dep in deps {
                self.find_cycles_dfs(dep, graph, visited, path, cycles);
            }
        }

        path.pop();
    }
}

/// 判断是否为入口点文件
fn is_entry_point(path: &str) -> bool {
    let file_name = Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    matches!(
        file_name,
        "main.rs" | "lib.rs" | "mod.rs" | "__init__.py" | "index.ts" | "index.js" | "main.go"
    )
}

/// 解析相对路径
fn resolve_relative_path(module_path: &str) -> String {
    let without_ext = module_path
        .trim_start_matches("/")
        .to_string();

    // Remove .ts, .js, .tsx, .jsx extension if present
    let without_ts = if without_ext.ends_with(".ts") || without_ext.ends_with(".tsx")
        || without_ext.ends_with(".js") || without_ext.ends_with(".jsx")
    {
        let last_dot = without_ext.rfind('.').unwrap_or(without_ext.len());
        without_ext[..last_dot].to_string()
    } else {
        without_ext
    };

    format!("{}.ts", without_ts) // default guess
}

/// 检测代码语言
pub fn detect_language(file_path: &str) -> Option<&'static str> {
    let ext = Path::new(file_path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();

    match ext.as_str() {
        "rs" => Some("rust"),
        "py" => Some("python"),
        "ts" | "tsx" => Some("typescript"),
        "js" | "jsx" => Some("javascript"),
        "go" => Some("go"),
        "java" => Some("java"),
        "kt" | "kts" => Some("kotlin"),
        "swift" => Some("swift"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_language() {
        assert_eq!(detect_language("main.rs"), Some("rust"));
        assert_eq!(detect_language("app.py"), Some("python"));
        assert_eq!(detect_language("component.tsx"), Some("typescript"));
        assert_eq!(detect_language("unknown.xyz"), None);
    }

    #[test]
    fn test_is_entry_point() {
        assert!(is_entry_point("src/main.rs"));
        assert!(is_entry_point("src/lib.rs"));
        assert!(is_entry_point("__init__.py"));
        assert!(!is_entry_point("src/util.rs"));
    }

    #[test]
    fn test_parse_rust_imports() {
        let analyzer = DependencyAnalyzer::new(Path::new("/tmp"));
        let content = r#"
use std::collections::HashMap;
use crate::module::submodule;
mod utils;
"#;
        let imports = analyzer.parse_rust_imports(content);
        assert!(imports.contains(&"std.rs".to_string()));
        assert!(imports.contains(&"src.rs".to_string())); // from crate::
        assert!(imports.contains(&"utils.rs".to_string()));
    }

    #[test]
    fn test_parse_python_imports() {
        let analyzer = DependencyAnalyzer::new(Path::new("/tmp"));
        let content = r#"
import os
from pathlib import Path
from .module import something
"#;
        let imports = analyzer.parse_python_imports(content);
        assert!(imports.contains(&"os.py".to_string()));
        assert!(imports.contains(&"pathlib/Path.py".to_string()));
        assert!(imports.contains(&".module.py".to_string()));
    }
}
