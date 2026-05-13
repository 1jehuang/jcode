//! # 文件引用解析
//! @mentions, 路径补全, 模糊搜索

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// @mention 提及解析结果
#[derive(Debug, Clone)]
pub struct FileRef {
    pub path: PathBuf, pub line: Option<u32>, pub column: Option<u32>,
    pub display: String, pub score: f64,
}

/// 解析 @file 或 #file 语法
pub fn parse_mention(text: &str, cwd: &Path) -> Vec<FileRef> {
    let mut refs = Vec::new();
    // 匹配 @路径 或 #路径 语法
    let re = regex::Regex::new(r"@([\w./\\-]+)|#([\w./\\-]+)").unwrap();
    for cap in re.captures_iter(text) {
        let raw = cap.get(1).or_else(|| cap.get(2)).map(|m| m.as_str()).unwrap_or("");
        let path = PathBuf::from(raw);
        let abs = if path.is_absolute() { path.clone() } else { cwd.join(&path) };
        let exists = abs.exists();
        refs.push(FileRef { path: abs, line: None, column: None, display: raw.to_string(), score: if exists { 1.0 } else { 0.3 } });
    }
    refs
}

/// 路径补全候选项
#[derive(Debug, Clone)]
pub struct PathCompletion { pub path: PathBuf, pub display: String, pub is_dir: bool }

/// 获取目录补全建议（LRU 缓存）
pub struct PathCompleter {
    cache: HashMap<PathBuf, (Instant, Vec<PathCompletion>)>,
    cache_ttl: Duration,
}

impl PathCompleter {
    pub fn new() -> Self { Self { cache: HashMap::new(), cache_ttl: Duration::from_secs(300) } }

    pub fn complete(&mut self, prefix: &str, base_dir: &Path) -> Vec<PathCompletion> {
        let dir = if prefix.contains('/') || prefix.contains('\\') {
            let p = Path::new(prefix);
            let parent = p.parent().unwrap_or(Path::new(""));
            let resolved = if parent.is_absolute() { parent.to_path_buf() } else { base_dir.join(parent) };
            (resolved, p.file_name().and_then(|s| s.to_str()).unwrap_or("").to_string())
        } else { (base_dir.to_path_buf(), prefix.to_string()) };

        // 检查缓存
        if let Some((time, cached)) = self.cache.get(&dir.0) && time.elapsed() < self.cache_ttl { return self.filter_cached(cached, &dir.1); }

        let mut results = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&dir.0) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                results.push(PathCompletion { path: entry.path(), display: name.clone(), is_dir });
            }
        }
        self.cache.insert(dir.0.clone(), (Instant::now(), results.clone()));
        self.filter_cached(&results, &dir.1)
    }

    fn filter_cached(&self, items: &[PathCompletion], prefix: &str) -> Vec<PathCompletion> {
        let lower = prefix.to_lowercase();
        items.iter().filter(|c| c.display.to_lowercase().starts_with(&lower)).cloned().collect()
    }
}

impl Default for PathCompleter { fn default() -> Self { Self::new() } }

/// 模糊文件搜索 (基于 walkdir)
pub fn fuzzy_search_files(root: &Path, query: &str, max_results: usize) -> Vec<PathBuf> {
    let lower_query = query.to_lowercase();
    let mut results = Vec::new();
    let walker = walkdir::WalkDir::new(root).max_depth(5).into_iter().filter_map(|e| e.ok());
    for entry in walker {
        if results.len() >= max_results { break; }
        let name = entry.file_name().to_string_lossy().to_lowercase();
        if name.contains(&lower_query) { results.push(entry.path().to_path_buf()); }
    }
    results
}
