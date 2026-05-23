//! # Memdir — 记忆目录系统（借鉴 Claude Code memdir/ 目录）
//!
//! 将记忆持久化为文件系统上的结构化目录。
//! 每个记忆条目是一个独立文件，按类别/时间分目录组织。
//!
//! ```
//! ~/.jcode/memdir/
//! ├── projects/
//! │   ├── carpai/
//! │   │   ├── 2026-05-23_architecture-decision.md
//! │   │   └── 2026-05-22_api-design.md
//! │   └── to-do-app/
//! │       └── 2026-05-20_setup.md
//! ├── patterns/
//! │   ├── rust-axum-best-practice.md
//! │   └── file-edit-algorithm.md
//! ├── errors/
//! │   └── rust-borrow-checker-solution.md
//! └── index.toml
//! ```

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// 记忆分类
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MemdirCategory {
    Project,
    Pattern,
    Error,
    Decision,
    Reference,
    Custom(String),
}

/// 记忆条目元数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemdirEntry {
    pub id: String,
    pub title: String,
    pub category: String,
    pub tags: Vec<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub relevance_score: f32,
    pub file_path: PathBuf,
}

/// 记忆目录索引
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MemdirIndex {
    pub entries: HashMap<String, MemdirEntry>,
    pub categories: Vec<String>,
    pub version: u32,
}

/// 记忆目录管理器
pub struct Memdir {
    root: PathBuf,
    index: std::sync::RwLock<MemdirIndex>,
}

impl Memdir {
    /// 创建或打开记忆目录
    pub fn new(base_path: &Path) -> Self {
        let root = base_path.join("memdir");
        std::fs::create_dir_all(&root).ok();
        std::fs::create_dir_all(root.join("projects")).ok();
        std::fs::create_dir_all(root.join("patterns")).ok();
        std::fs::create_dir_all(root.join("errors")).ok();
        std::fs::create_dir_all(root.join("decisions")).ok();

        let index_path = root.join("index.toml");
        let index = std::fs::read_to_string(&index_path)
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default();

        Self {
            root,
            index: std::sync::RwLock::new(index),
        }
    }

    /// 写入一条记忆
    pub fn write(&self, title: &str, content: &str, category: &str, tags: Vec<String>) -> std::io::Result<String> {
        let date = chrono::Utc::now().format("%Y-%m-%d");
        let safe_title = title.replace(' ', "-").to_lowercase();
        let filename = format!("{}_{}.md", date, safe_title);
        let category_dir = self.root.join(category);
        std::fs::create_dir_all(&category_dir)?;
        let filepath = category_dir.join(&filename);

        let frontmatter = format!(
            "---\ntitle: {}\ncreated: {}\ncategory: {}\ntags: [{}]\n---\n\n",
            title,
            chrono::Utc::now().to_rfc3339(),
            category,
            tags.join(", ")
        );
        std::fs::write(&filepath, frontmatter + content)?;

        let id = format!("{}-{}", category, safe_title);
        let mut index = self.index.write().unwrap();
        index.entries.insert(id.clone(), MemdirEntry {
            id: id.clone(),
            title: title.to_string(),
            category: category.to_string(),
            tags,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            relevance_score: 1.0,
            file_path: filepath,
        });

        // 更新索引文件
        let index_toml = toml::to_string_pretty(&*index).unwrap_or_default();
        std::fs::write(self.root.join("index.toml"), index_toml).ok();

        Ok(id)
    }

    /// 搜索记忆
    pub fn search(&self, query: &str, category: Option<&str>) -> Vec<MemdirEntry> {
        let index = self.index.read().unwrap();
        let query_lower = query.to_lowercase();
        index.entries
            .values()
            .filter(|e| {
                let cat_match = category.map(|c| e.category == c).unwrap_or(true);
                let query_match = e.title.to_lowercase().contains(&query_lower)
                    || e.tags.iter().any(|t| t.to_lowercase().contains(&query_lower));
                cat_match && query_match
            })
            .cloned()
            .collect()
    }

    /// 读取一条记忆内容
    pub fn read(&self, id: &str) -> Option<String> {
        let index = self.index.read().unwrap();
        let entry = index.entries.get(id)?;
        std::fs::read_to_string(&entry.file_path).ok()
    }

    /// 列出所有记忆（按类别分组）
    pub fn list_by_category(&self) -> HashMap<String, Vec<MemdirEntry>> {
        let index = self.index.read().unwrap();
        let mut result: HashMap<String, Vec<MemdirEntry>> = HashMap::new();
        for entry in index.entries.values() {
            result.entry(entry.category.clone()).or_default().push(entry.clone());
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memdir_write_and_search() {
        let dir = tempfile::tempdir().unwrap();
        let memdir = Memdir::new(dir.path());

        let id = memdir.write("测试记忆", "这是测试内容", "projects", vec!["rust".into()]).unwrap();
        assert!(id.contains("projects"));

        let results = memdir.search("测试", None);
        assert_eq!(results.len(), 1);

        let results = memdir.search("rust", None);
        assert_eq!(results.len(), 1);

        let results = memdir.search("不存在的", None);
        assert_eq!(results.len(), 0);

        let content = memdir.read(&id);
        assert!(content.is_some());
        assert!(content.unwrap().contains("测试内容"));
    }
}
