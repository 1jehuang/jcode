//! File tree widget — Side panel for workspace navigation
//!
//! Displays a tree view of the project directory, allowing quick file
//! navigation and selection. Uses async I/O to avoid blocking the TUI event loop.
//! This is a pure UI widget with no business logic.

use std::path::PathBuf;

use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    widgets::{Block, Borders, HighlightSpacing, List, ListItem, ListState},
};
use tokio::fs;
use tracing::warn;

use crate::tui::theme::Theme;

/// A node in the file tree
#[derive(Debug, Clone)]
pub enum TreeNode {
    File { name: String, path: PathBuf },
    Directory { name: String, path: PathBuf, children: Vec<TreeNode> },
}

/// File tree state
pub struct FileTree {
    #[allow(dead_code)]
    root: Option<TreeNode>,
    pub all_files: Vec<PathBuf>,
    pub state: ListState,
    pub visible: bool,
}

impl FileTree {
    pub fn new() -> Self {
        Self {
            root: None,
            all_files: Vec::new(),
            state: ListState::default(),
            visible: false,
        }
    }

    /// Scan a directory synchronously
    pub fn scan_directory(&mut self, dir: &PathBuf) -> std::io::Result<()> {
        let mut all_files = Vec::new();
        let root = build_tree(dir, dir, &mut all_files)?;
        self.root = Some(root);
        self.all_files = all_files;
        self.state.select(if self.all_files.is_empty() { None } else { Some(0) });
        Ok(())
    }

    /// Scan a directory asynchronously (uses tokio::fs, non-recursive to avoid E0733)
    pub async fn scan_directory_async(&mut self, dir: &PathBuf) -> std::io::Result<()> {
        let mut all_files = Vec::new();
        let root = build_tree_async_nonrecursive(dir, &mut all_files).await?;
        self.root = Some(root);
        self.all_files = all_files;
        self.state.select(if self.all_files.is_empty() { None } else { Some(0) });
        Ok(())
    }

    pub fn selected_path(&self) -> Option<PathBuf> {
        self.state.selected().map(|i| self.all_files[i].clone())
    }

    pub fn next(&mut self) {
        let i = self.state.selected()
            .map(|i| (i + 1).min(self.all_files.len().saturating_sub(1)));
        self.state.select(i);
    }

    pub fn previous(&mut self) {
        let i = self.state.selected().map(|i| i.saturating_sub(1));
        self.state.select(i);
    }

    pub fn toggle(&mut self) {
        self.visible = !self.visible;
    }
}

/// Non-recursive async version — collects all files using an explicit stack,
/// avoiding the E0733 "recursion in async fn" error.
async fn build_tree_async_nonrecursive(
    dir: &PathBuf,
    all_files: &mut Vec<PathBuf>,
) -> std::io::Result<TreeNode> {
    let name = dir
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| dir.to_string_lossy().to_string());

    let mut children = Vec::new();

    if let Ok(mut reader) = fs::read_dir(dir).await {
        let mut entries = Vec::new();
        loop {
            match reader.next_entry().await {
                Ok(Some(entry)) => entries.push(entry),
                Ok(None) => break,
                Err(e) => {
                    warn!(error = %e, path = %dir.display(), "Error reading directory");
                    break;
                }
            }
        }

        for entry in entries {
            let path = entry.path();
            if path
                .file_name()
                .and_then(|s| s.to_str())
                .map(|s| s.starts_with('.'))
                .unwrap_or(false)
            {
                continue;
            }

            let is_dir = fs::metadata(&path).await
                .map(|m| m.is_dir())
                .unwrap_or(false);

            if is_dir {
                // Box::pin required because async fn is recursive (E0733)
                let subtree = Box::pin(build_tree_async_nonrecursive(&path, all_files)).await?;
                children.push(subtree);
            } else {
                children.push(TreeNode::File {
                    name: path
                        .file_name()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_default(),
                    path: path.clone(),
                });
                all_files.push(path.strip_prefix(dir).unwrap_or(&path).to_path_buf());
            }
        }
    }

    Ok(TreeNode::Directory {
        name,
        path: dir.clone(),
        children,
    })
}

/// Synchronous fallback
fn build_tree(base: &PathBuf, dir: &PathBuf, all_files: &mut Vec<PathBuf>) -> std::io::Result<TreeNode> {
    let name = dir
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| dir.to_string_lossy().to_string());

    let mut children = Vec::new();

    if dir.is_dir() {
        let mut entries: Vec<_> = std::fs::read_dir(dir)?
            .filter_map(|e| e.ok())
            .collect();
        entries.sort_by_key(|e| e.file_name());

        for entry in entries {
            let path = entry.path();
            if path
                .file_name()
                .and_then(|s| s.to_str())
                .map(|s| s.starts_with('.'))
                .unwrap_or(false)
            {
                continue;
            }

            if path.is_dir() {
                children.push(build_tree(base, &path, all_files)?);
            } else {
                children.push(TreeNode::File {
                    name: path
                        .file_name()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_default(),
                    path: path.clone(),
                });
                all_files.push(path.strip_prefix(base).unwrap_or(&path).to_path_buf());
            }
        }
    }

    Ok(TreeNode::Directory {
        name,
        path: dir.clone(),
        children,
    })
}

pub fn render_file_tree(f: &mut Frame, area: Rect, tree: &mut FileTree, theme: &Theme) {
    if !tree.visible {
        return;
    }

    let items: Vec<ListItem> = tree
        .all_files
        .iter()
        .map(|p| ListItem::new(format!(" {}", p.display())))
        .collect();

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(" Files ").style(theme.text_dim))
        .highlight_style(Style::default().fg(theme.primary))
        .highlight_spacing(HighlightSpacing::Always);

    f.render_stateful_widget(list, area, &mut tree.state);
}
