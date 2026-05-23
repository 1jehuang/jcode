//! W1: Project Scanner — 扫描项目文件，识别语言/框架
//! 移植自: Understand-Anything agents/project-scanner
//! 确定性解析器: 不依赖 LLM，纯文件系统扫描

use std::collections::HashSet;
use std::path::Path;
use ignore::WalkBuilder;

use super::{KnowledgeGraph, PipelineConfig, ComplexityLevel, KGNode, NodeKind, KGEdge, RelationType, ArchitectureLayer};

/// 扫描结果
#[derive(Debug, Clone)]
pub struct FileEntry {
    pub path: String,
    pub language: String,
    pub framework: Option<String>,
    pub size_bytes: u64,
    pub lines: usize,
    pub is_binary: bool,
}

/// 检测语言
pub fn detect_language(path: &str) -> Option<&'static str> {
    let ext = path.rsplit('.').next()?;
    match ext {
        "rs" => Some("Rust"),
        "ts" | "tsx" => Some("TypeScript"),
        "js" | "jsx" => Some("JavaScript"),
        "py" => Some("Python"),
        "go" => Some("Go"),
        "java" => Some("Java"),
        "kt" | "kts" => Some("Kotlin"),
        "swift" => Some("Swift"),
        "rb" => Some("Ruby"),
        "php" => Some("PHP"),
        "c" | "h" => Some("C"),
        "cpp" | "hpp" | "cc" => Some("C++"),
        "cs" => Some("C#"),
        "rs"  => Some("Rust"),
        "vue" => Some("Vue"),
        "svelte" => Some("Svelte"),
        "css" | "scss" | "less" => Some("CSS"),
        "html" | "htm" => Some("HTML"),
        "sql" => Some("SQL"),
        "yaml" | "yml" => Some("YAML"),
        "json" => Some("JSON"),
        "toml" => Some("TOML"),
        "md" | "mdx" => Some("Markdown"),
        "dockerfile" | "Dockerfile" => Some("Docker"),
        "tf" => Some("Terraform"),
        "sh" | "bash" => Some("Shell"),
        "ps1" => Some("PowerShell"),
        "lua" => Some("Lua"),
        _ => None,
    }
}

/// 检测框架 (基于配置文件或目录结构)
pub fn detect_framework(root: &Path) -> Vec<String> {
    let mut frameworks = Vec::new();

    // Cargo.toml → Rust workspace
    if root.join("Cargo.toml").exists() {
        frameworks.push("Cargo/Rust".to_string());
        if root.join("Cargo.lock").exists() {
            frameworks.push("Cargo Workspace".to_string());
        }
    }

    // package.json → Node/JS
    let pkg_json = root.join("package.json");
    if pkg_json.exists() {
        if let Ok(content) = std::fs::read_to_string(&pkg_json) {
            if content.contains("\"next\"") { frameworks.push("Next.js".to_string()); }
            if content.contains("\"react\"") { frameworks.push("React".to_string()); }
            if content.contains("\"vue\"") { frameworks.push("Vue".to_string()); }
            if content.contains("\"svelte\"") { frameworks.push("Svelte".to_string()); }
            if content.contains("\"express\"") { frameworks.push("Express".to_string()); }
            if content.contains("\"nestjs\"") || content.contains("\"nest\"") {
                frameworks.push("NestJS".to_string());
            }
        }
    }

    // pyproject.toml / requirements.txt → Python
    if root.join("pyproject.toml").exists() || root.join("requirements.txt").exists() {
        frameworks.push("Python".to_string());
        if root.join("Django").is_dir() || root.join("manage.py").exists() {
            frameworks.push("Django".to_string());
        }
    }

    // go.mod → Go
    if root.join("go.mod").exists() {
        frameworks.push("Go Modules".to_string());
    }

    // Dockerfile
    if root.join("Dockerfile").exists() {
        frameworks.push("Docker".to_string());
    }

    // .github/workflows → GitHub Actions
    if root.join(".github").join("workflows").exists() {
        frameworks.push("GitHub Actions".to_string());
    }

    frameworks
}

/// 扫描项目文件 (Agent 1)
/// 跳过: .git, node_modules, target, build, dist, .venv, .next, __pycache__
pub async fn scan_project(root: &Path, config: &PipelineConfig) -> Result<Vec<FileEntry>, String> {
    let mut files = Vec::new();

    let walker = WalkBuilder::new(root)
        .ignore(true)       // 遵守 .gitignore
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .max_depth(Some(20))
        .standard_filters(true)
        .build();

    let ignored_dirs: HashSet<&str> = [
        "node_modules", "target", "build", "dist", ".next", ".venv",
        "__pycache__", ".git", ".svn", ".idea", ".vscode",
        "vendor", "third_party", "third-party", ".cargo",
    ].iter().cloned().collect();

    let ignored_extensions: HashSet<&str> = [
        "png", "jpg", "jpeg", "gif", "ico", "svg",
        "woff", "woff2", "ttf", "eot",
        "mp4", "mp3", "avi", "mov",
        "zip", "tar", "gz", "rar",
        "o", "so", "dll", "dylib", "exe",
        "lock", "min.js", "min.css",
    ].iter().cloned().collect();

    for entry in walker {
        let entry = entry.map_err(|e| format!("Walk error: {}", e))?;
        let path = entry.path();

        // 跳过目录
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            let dir_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if ignored_dirs.contains(dir_name) {
                continue;
            }
            continue;
        }

        // 获取相对路径
        let rel_path = path.strip_prefix(root).unwrap_or(path);
        let rel_str = rel_path.to_string_lossy().to_string();

        // 跳过忽略的扩展名
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if ignored_extensions.contains(ext) {
            continue;
        }

        // 检测语言
        let lang = detect_language(&rel_str);
        if lang.is_none() && ext != "rs" && !rel_str.ends_with(".rs") {
            continue; // 只保留已知语言的源文件
        }

        // 读取文件基本信息
        let metadata = tokio::fs::metadata(path).await
            .map_err(|e| format!("Metadata error {}: {}", rel_str, e))?;
        let size = metadata.len();

        // 行数估算
        let lines = if size < 1_000_000 {
            let content = tokio::fs::read_to_string(path).await.unwrap_or_default();
            content.lines().count()
        } else { 0 };

        // 检测二进制
        let is_binary = !ext.is_empty() && ["png", "jpg", "gif", "ico", "woff", "eot", "ttf", "zip", "rar", "gz"]
            .contains(&ext);

        files.push(FileEntry {
            path: rel_str,
            language: lang.unwrap_or("Unknown").to_string(),
            framework: None,
            size_bytes: size,
            lines,
            is_binary,
        });
    }

    // 排序: 按路径
    files.sort_by(|a, b| a.path.cmp(&b.path));

    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_language() {
        assert_eq!(detect_language("main.rs"), Some("Rust"));
        assert_eq!(detect_language("app.ts"), Some("TypeScript"));
        assert_eq!(detect_language("app.tsx"), Some("TypeScript"));
        assert_eq!(detect_language("test.py"), Some("Python"));
        assert_eq!(detect_language("module.js"), Some("JavaScript"));
        assert_eq!(detect_language("styles.css"), Some("CSS"));
        assert_eq!(detect_language("schema.sql"), Some("SQL"));
        assert_eq!(detect_language("Dockerfile"), Some("Docker"));
    }

    #[test]
    fn test_detect_framework() {
        let temp = std::env::temp_dir().join("test-framework-detect");
        let _ = std::fs::create_dir_all(&temp);
        std::fs::write(temp.join("Cargo.toml"), "[package]\nname = \"test\"\n").ok();
        let fw = detect_framework(&temp);
        assert!(fw.contains(&"Cargo/Rust".to_string()));
        let _ = std::fs::remove_dir_all(&temp);
    }
}
