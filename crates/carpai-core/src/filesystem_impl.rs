use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::future::Future;
use async_trait::async_trait;
use tokio::fs;
use carpai_internal::*;
use tracing::{debug, warn};
use sha2::{Sha256, Digest};

pub struct LocalFileSystem {
    working_dir: PathBuf,
    vfs_root: Option<PathBuf>,
}

impl LocalFileSystem {
    pub fn new(working_dir: &Path, vfs_root: Option<&Path>) -> Self {
        Self {
            working_dir: working_dir.to_path_buf(),
            vfs_root: vfs_root.map(|p| p.to_path_buf()),
        }
    }

    fn resolve_path(&self, path: &Path) -> PathBuf {
        if let Some(ref vfs) = self.vfs_root {
            vfs.join(path)
        } else {
            self.working_dir.join(path)
        }
    }

    fn to_file_meta(&self, full_path: &Path, rel_path: &Path) -> FileMeta {
        let meta = std::fs::metadata(full_path).ok();
        
        match meta {
            Some(m) => FileMeta {
                path: rel_path.to_path_buf(),
                size: m.len(),
                is_dir: m.is_dir(),
                is_symlink: m.is_symlink(),
                modified_at: m.modified()
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH),
                created_at: m.created().ok(),
                extension: rel_path.extension()
                    .map(|e| e.to_string_lossy().to_string()),
                content_hash: None,
            },
            None => FileMeta {
                path: rel_path.to_path_buf(),
                size: 0,
                is_dir: false,
                is_symlink: false,
                modified_at: std::time::SystemTime::UNIX_EPOCH,
                created_at: None,
                extension: rel_path.extension()
                    .map(|e| e.to_string_lossy().to_string()),
                content_hash: None,
            },
        }
    }
}

#[async_trait]
impl VirtualFileSystem for LocalFileSystem {
    async fn read_file(&self, path: &Path) -> Result<String, FsError> {
        let full_path = self.resolve_path(path);

        if !full_path.exists() {
            return Err(FsError::NotFound(full_path.display().to_string()));
        }

        if full_path.is_dir() {
            return Err(FsError::NotAFile(full_path.display().to_string()));
        }

        fs::read_to_string(&full_path)
            .await
            .map_err(|e| FsError::Io(e))
    }

    async fn read_file_bytes(&self, path: &Path) -> Result<Vec<u8>, FsError> {
        let full_path = self.resolve_path(path);

        if !full_path.exists() {
            return Err(FsError::NotFound(full_path.display().to_string()));
        }

        if full_path.is_dir() {
            return Err(FsError::NotAFile(full_path.display().to_string()));
        }

        fs::read(&full_path)
            .await
            .map_err(|e| FsError::Io(e))
    }

    async fn write_file(&self, path: &Path, content: &str) -> Result<FileWriteResult, FsError> {
        let full_path = self.resolve_path(path);

        if let Some(parent) = full_path.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|e| FsError::Io(e))?;
        }

        let existed_before = full_path.exists();
        let previous_hash = if existed_before {
            fs::read(&full_path)
                .await
                .ok()
                .map(|data| format!("{:x}", Sha256::digest(data)))
        } else {
            None
        };

        fs::write(&full_path, content)
            .await
            .map_err(|e| FsError::Io(e))?;

        let new_hash = format!("{:x}", Sha256::digest(content.as_bytes()));
        let bytes_written = content.len() as u64;

        debug!(path = %full_path.display(), bytes = bytes_written, "File written");

        Ok(FileWriteResult {
            bytes_written,
            created: !existed_before,
            audit_id: None,
            previous_hash,
            new_hash,
        })
    }

    async fn write_file_bytes(&self, path: &Path, data: &[u8]) -> Result<FileWriteResult, FsError> {
        let full_path = self.resolve_path(path);

        if let Some(parent) = full_path.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|e| FsError::Io(e))?;
        }

        let existed_before = full_path.exists();
        let previous_hash = if existed_before {
            fs::read(&full_path)
                .await
                .ok()
                .map(|d| format!("{:x}", Sha256::digest(d)))
        } else {
            None
        };

        fs::write(&full_path, data)
            .await
            .map_err(|e| FsError::Io(e))?;

        let new_hash = format!("{:x}", Sha256::digest(data));

        debug!(path = %full_path.display(), bytes = data.len(), "File written (bytes)");

        Ok(FileWriteResult {
            bytes_written: data.len() as u64,
            created: !existed_before,
            audit_id: None,
            previous_hash,
            new_hash,
        })
    }

    async fn delete_file(&self, path: &Path) -> Result<(), FsError> {
        let full_path = self.resolve_path(path);

        if !full_path.exists() {
            return Err(FsError::NotFound(full_path.display().to_string()));
        }

        if full_path.is_dir() {
            return Err(FsError::NotAFile(full_path.display().to_string()));
        }

        fs::remove_file(&full_path)
            .await
            .map_err(|e| FsError::Io(e))?;

        debug!(path = %full_path.display(), "File deleted");
        Ok(())
    }

    async fn exists(&self, path: &Path) -> Result<bool, FsError> {
        let full_path = self.resolve_path(path);
        Ok(full_path.exists())
    }

    async fn metadata(&self, path: &Path) -> Result<FileMeta, FsError> {
        let full_path = self.resolve_path(path);

        if !full_path.exists() {
            return Err(FsError::NotFound(full_path.display().to_string()));
        }

        let meta = fs::metadata(&full_path)
            .await
            .map_err(|e| FsError::Io(e))?;

        Ok(FileMeta {
            path: path.to_path_buf(),
            size: meta.len(),
            is_dir: meta.is_dir(),
            is_symlink: meta.is_symlink(),
            modified_at: meta.modified()
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH),
            created_at: meta.created().ok(),
            extension: path.extension()
                .map(|e| e.to_string_lossy().to_string()),
            content_hash: None,
        })
    }

    async fn list_dir(
        &self,
        path: &Path,
        recursive: bool,
    ) -> Result<Vec<FileEntry>, FsError> {
        let full_path = self.resolve_path(path);

        if !full_path.exists() {
            return Err(FsError::NotFound(full_path.display().to_string()));
        }

        if !full_path.is_dir() {
            return Err(FsError::NotADirectory(full_path.display().to_string()));
        }

        let entries = self.collect_entries(&full_path, path, recursive).await;

        Ok(entries)
    }

    async fn create_dir(&self, path: &Path) -> Result<(), FsError> {
        let full_path = self.resolve_path(path);

        fs::create_dir_all(&full_path)
            .await
            .map_err(|e| FsError::Io(e))?;

        debug!(path = %full_path.display(), "Directory created");
        Ok(())
    }

    async fn delete_dir(&self, path: &Path, recursive: bool) -> Result<(), FsError> {
        let full_path = self.resolve_path(path);

        if !full_path.exists() {
            return Err(FsError::NotFound(full_path.display().to_string()));
        }

        if !full_path.is_dir() {
            return Err(FsError::NotADirectory(full_path.display().to_string()));
        }

        if recursive {
            fs::remove_dir_all(&full_path)
                .await
                .map_err(|e| FsError::Io(e))?;
        } else {
            fs::remove_dir(&full_path)
                .await
                .map_err(|e| {
                    if e.kind() == std::io::ErrorKind::DirectoryNotEmpty ||
                       e.raw_os_error() == Some(145) {
                        FsError::NotEmpty(full_path.display().to_string())
                    } else {
                        FsError::Io(e)
                    }
                })?;
        }

        debug!(path = %full_path.display(), recursive, "Directory deleted");
        Ok(())
    }

    async fn search_files(
        &self,
        pattern: &str,
        in_path: &Path,
        max_results: usize,
    ) -> Result<Vec<SearchResult>, FsError> {
        let base_path = self.resolve_path(in_path);
        let glob_pattern = if pattern.contains('*') || pattern.contains('?') {
            pattern.to_string()
        } else {
            format!("**/*{}*", pattern)
        };

        let mut results = Vec::new();

        for entry in walk_dir_recursive(&base_path).await {
            let entry_path = entry.path();
            let rel_path = entry_path
                .strip_prefix(&base_path)
                .unwrap_or(&entry_path);
            
            let file_name = entry_path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();

            let matches_pattern = if pattern.contains('*') || pattern.contains('?') {
                simple_glob_match(&glob_pattern, &file_name)
            } else {
                file_name.to_lowercase().contains(&pattern.to_lowercase())
            };

            if matches_pattern {
                results.push(SearchResult {
                    path: rel_path.to_path_buf(),
                    meta: self.to_file_meta(&entry_path, rel_path),
                    score: 1.0,
                });

                if results.len() >= max_results {
                    break;
                }
            }
        }

        Ok(results)
    }

    async fn search_content(
        &self,
        query: &str,
        in_path: &Path,
        options: SearchOptions,
    ) -> Result<Vec<ContentMatch>, FsError> {
        let base_path = self.resolve_path(in_path);
        let all_entries = walk_dir_recursive(&base_path).await;

        let mut all_matches = Vec::new();

        for entry in all_entries {
            let entry_path = entry.path();
            
            if !options.extensions.is_empty() {
                let ext = entry_path
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("");
                if !options.extensions.iter().any(|e| e == ext) {
                    continue;
                }
            }

            let exclude = options.exclude_patterns.iter().any(|pat| {
                let file_name = entry_path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                simple_glob_match(pat, &file_name)
            });
            if exclude {
                continue;
            }

            let content = match fs::read_to_string(&entry_path).await {
                Ok(c) => c,
                Err(_) => continue,
            };

            let rel_path = entry_path
                .strip_prefix(&base_path)
                .unwrap_or(&entry_path);

            let file_matches = find_content_matches(
                rel_path,
                &content,
                query,
                &options,
            );

            all_matches.extend(file_matches);

            if all_matches.len() >= options.max_matches_per_file * 100 {
                break;
            }
        }

        Ok(all_matches)
    }

    async fn git_diff(&self, _path: &Path, _staged: bool) -> Result<String, FsError> {
        Err(FsError::Unsupported)
    }

    async fn git_status(&self, _path: &Path) -> Result<String, FsError> {
        Err(FsError::Unsupported)
    }

    async fn git_blame(&self, _path: &Path) -> Result<String, FsError> {
        Err(FsError::Unsupported)
    }

    async fn watch(
        &self,
        _path: &Path,
    ) -> Result<Pin<Box<dyn tokio_stream::Stream<Item = FsEvent> + Send>>, FsError> {
        Err(FsError::Unsupported)
    }

    fn resolve(&self, path: &Path) -> Result<PathBuf, FsError> {
        let resolved = self.resolve_path(path);
        let root = self.root();
        if !resolved.starts_with(root) {
            return Err(FsError::PathEscape {
                path: resolved.display().to_string(),
                root: root.display().to_string(),
            });
        }
        Ok(resolved)
    }

    fn root(&self) -> &Path {
        self.vfs_root.as_deref().unwrap_or(&self.working_dir)
    }

    fn is_allowed(&self, path: &Path) -> bool {
        match self.resolve(path) {
            Ok(_) => true,
            Err(_) => false,
        }
    }
}

impl LocalFileSystem {
    fn collect_entries<'a>(
        &'a self,
        full_path: &'a Path,
        rel_base: &'a Path,
        recursive: bool,
    ) -> Pin<Box<dyn Future<Output = Vec<FileEntry>> + Send + 'a>> {
        Box::pin(async move {
            let mut entries = Vec::new();

            let mut dir = match fs::read_dir(full_path).await {
                Ok(d) => d,
                Err(_) => return entries,
            };

            while let Ok(Some(entry)) = dir.next_entry().await {
                let entry_path = entry.path();
                let rel_path = entry_path
                    .strip_prefix(rel_base)
                    .unwrap_or(&entry_path);

                let name = entry_path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();

                entries.push(FileEntry {
                    name: name.clone(),
                    path: rel_path.to_path_buf(),
                    meta: self.to_file_meta(&entry_path, rel_path),
                });

                if recursive && entry_path.is_dir() {
                    entries.extend(self.collect_entries(&entry_path, rel_base, true).await);
                }
            }

            entries.sort_by(|a, b| {
                match (a.meta.is_dir, b.meta.is_dir) {
                    (true, false) => std::cmp::Ordering::Less,
                    (false, true) => std::cmp::Ordering::Greater,
                    _ => a.name.cmp(&b.name),
                }
            });

            entries
        })
    }
}

fn walk_dir_recursive(root: &Path) -> Pin<Box<dyn Future<Output = Vec<tokio::fs::DirEntry>> + Send + '_>> {
    Box::pin(async move {
        let mut entries = Vec::new();
        if let Ok(mut dir) = fs::read_dir(root).await {
            while let Ok(Some(entry)) = dir.next_entry().await {
                let path = entry.path();
                if path.is_dir() {
                    entries.extend(walk_dir_recursive(&path).await);
                } else {
                    entries.push(entry);
                }
            }
        }
        entries
    })
}

fn simple_glob_match(pattern: &str, text: &str) -> bool {
    let pattern_lower = pattern.to_lowercase();
    let text_lower = text.to_lowercase();
    
    if pattern_lower == "*" {
        return true;
    }

    let parts: Vec<&str> = pattern_lower.split('*').collect();
    if parts.len() == 1 {
        return text_lower == parts[0];
    }

    if !text_lower.starts_with(parts[0]) {
        return false;
    }
    if !parts.last().map(|p| p.is_empty() || text_lower.ends_with(p)).unwrap_or(true) {
        return false;
    }

    let mut remaining = &text_lower[parts[0].len()..];
    for part in &parts[1..parts.len() - 1] {
        if part.is_empty() {
            continue;
        }
        if let Some(pos) = remaining.find(part) {
            remaining = &remaining[pos + part.len()..];
        } else {
            return false;
        }
    }

    true
}

fn find_content_matches(
    file: &Path,
    content: &str,
    query: &str,
    options: &SearchOptions,
) -> Vec<ContentMatch> {
    let mut matches = Vec::new();
    let lines: Vec<&str> = content.lines().collect();

    let search_fn: Box<dyn Fn(&str) -> bool> = if options.regex {
        match regex::Regex::new(query) {
            Ok(re) => Box::new(move |line: &str| re.is_match(line)),
            Err(_) => Box::new(move |line: &str| line.contains(query)),
        }
    } else if options.case_insensitive {
        let q = query.to_lowercase();
        Box::new(move |line: &str| line.to_lowercase().contains(&q))
    } else {
        let q = query.to_string();
        Box::new(move |line: &str| line.contains(&q))
    };

    for (idx, line) in lines.iter().enumerate() {
        if search_fn(line) {
            let line_num = idx + 1;
            let byte_offset = content.lines().take(idx).map(|l| l.len() + 1).sum::<usize>();
            let match_start = if options.case_insensitive {
                line.to_lowercase().find(&query.to_lowercase()).unwrap_or(0)
            } else {
                line.find(query).unwrap_or(0)
            };

            let before: Vec<String> = lines[..idx]
                .iter()
                .rev()
                .take(options.context_lines_before)
                .map(|l| l.to_string())
                .collect();
            let before = before.into_iter().rev().collect();

            let after: Vec<String> = lines[idx + 1..]
                .iter()
                .take(options.context_lines_after)
                .map(|l| l.to_string())
                .collect();

            matches.push(ContentMatch {
                file: file.to_path_buf(),
                line_number: line_num,
                line: (*line).to_string(),
                byte_offset: byte_offset + match_start,
                match_length: query.len(),
                before_context: before,
                after_context: after,
            });

            if matches.len() >= options.max_matches_per_file {
                break;
            }
        }
    }

    matches
}
