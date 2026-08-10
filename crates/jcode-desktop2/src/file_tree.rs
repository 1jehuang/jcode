//! Project file tree state.
//!
//! The tree is a deliberately small snapshot of the attached session's working
//! directory. Directories are read only when expanded, keeping startup and large
//! repositories cheap while still making the explorer immediately useful.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub const WIDTH: f64 = 252.0;
pub const HEADER_HEIGHT: f64 = 58.0;
pub const ROW_HEIGHT: f64 = 24.0;
pub const INDENT: f64 = 16.0;
pub const MAX_CHILDREN: usize = 500;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub directory: bool,
}

#[derive(Clone, Debug, Default)]
pub struct FileTree {
    root: Option<PathBuf>,
    expanded: BTreeSet<PathBuf>,
}

impl FileTree {
    pub fn sync_root(&mut self, root: Option<&str>) {
        let root = root.map(PathBuf::from);
        if self.root == root {
            return;
        }
        self.root = root;
        self.expanded.clear();
        if let Some(root) = self.root.clone() {
            self.expanded.insert(root);
        }
    }

    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    pub fn root_label(&self) -> &str {
        self.root
            .as_deref()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or("FILES")
    }

    pub fn toggle(&mut self, path: &Path) {
        if !self.expanded.remove(path) {
            self.expanded.insert(path.to_path_buf());
        }
    }

    pub fn is_expanded(&self, path: &Path) -> bool {
        self.expanded.contains(path)
    }

    pub fn visible(&self, max_rows: usize) -> Vec<(Entry, usize)> {
        let Some(root) = self.root.as_deref() else {
            return Vec::new();
        };
        let mut rows = Vec::new();
        self.collect(root, 0, max_rows, &mut rows);
        rows
    }

    fn collect(&self, dir: &Path, depth: usize, max: usize, rows: &mut Vec<(Entry, usize)>) {
        if rows.len() >= max || !self.is_expanded(dir) {
            return;
        }
        let Ok(read) = std::fs::read_dir(dir) else {
            return;
        };
        let mut children: Vec<Entry> = read
            .filter_map(Result::ok)
            .filter_map(|item| {
                let name = item.file_name().to_string_lossy().into_owned();
                if name == ".git" || name == "target" || name == "node_modules" {
                    return None;
                }
                let directory = item.file_type().ok()?.is_dir();
                Some(Entry {
                    path: item.path(),
                    name,
                    directory,
                })
            })
            .take(MAX_CHILDREN)
            .collect();
        children.sort_by_key(|entry| (!entry.directory, entry.name.to_lowercase()));
        for child in children {
            if rows.len() >= max {
                break;
            }
            let descend = child.directory && self.is_expanded(&child.path);
            let path = child.path.clone();
            rows.push((child, depth));
            if descend {
                self.collect(&path, depth + 1, max, rows);
            }
        }
    }

    pub fn row_at(&self, y: f64, height: f64) -> Option<Entry> {
        if y < HEADER_HEIGHT || y >= height {
            return None;
        }
        let max = ((height - HEADER_HEIGHT) / ROW_HEIGHT).floor().max(0.0) as usize;
        self.visible(max)
            .get(((y - HEADER_HEIGHT) / ROW_HEIGHT) as usize)
            .map(|(entry, _)| entry.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn no_root_has_no_rows() {
        assert!(FileTree::default().visible(20).is_empty());
    }
}
