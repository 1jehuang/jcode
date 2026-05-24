use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Mutex;
use async_trait::async_trait;
use carpai_internal::*;
use sha2::Digest;

pub struct MockFileSystem {
    files: Mutex<HashMap<PathBuf, String>>,
    root_path: PathBuf,
}

impl MockFileSystem {
    pub fn new() -> Self {
        Self {
            files: Mutex::new(HashMap::new()),
            root_path: PathBuf::from("/mock"),
        }
    }

    pub fn add_file(&self, path: &str, content: &str) {
        self.files.lock().unwrap().insert(PathBuf::from(path), content.into());
    }
}

impl Default for MockFileSystem {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl VirtualFileSystem for MockFileSystem {
    async fn read_file(&self, path: &Path) -> Result<String, FsError> {
        let files = self.files.lock().unwrap();
        files.get(path).cloned().ok_or_else(|| FsError::NotFound(path.display().to_string()))
    }

    async fn read_file_bytes(&self, path: &Path) -> Result<Vec<u8>, FsError> {
        let content = self.read_file(path).await?;
        Ok(content.into_bytes())
    }

    async fn write_file(&self, path: &Path, content: &str) -> Result<FileWriteResult, FsError> {
        let mut files = self.files.lock().unwrap();
        let created = !files.contains_key(path);
        files.insert(path.to_path_buf(), content.into());
        Ok(FileWriteResult {
            bytes_written: content.len() as u64,
            created,
            audit_id: None,
            previous_hash: None,
            new_hash: format!("{:x}", sha2::Sha256::digest(content.as_bytes())),
        })
    }

    async fn write_file_bytes(&self, path: &Path, data: &[u8]) -> Result<FileWriteResult, FsError> {
        let content = String::from_utf8_lossy(data).into_owned();
        self.write_file(path, &content).await
    }

    async fn delete_file(&self, path: &Path) -> Result<(), FsError> {
        let mut files = self.files.lock().unwrap();
        files.remove(path).ok_or_else(|| FsError::NotFound(path.display().to_string()))?;
        Ok(())
    }

    async fn exists(&self, path: &Path) -> Result<bool, FsError> {
        let files = self.files.lock().unwrap();
        Ok(files.contains_key(path))
    }

    async fn metadata(&self, path: &Path) -> Result<FileMeta, FsError> {
        let files = self.files.lock().unwrap();
        if let Some(content) = files.get(path) {
            Ok(FileMeta {
                path: path.to_path_buf(),
                size: content.len() as u64,
                is_dir: false,
                is_symlink: false,
                modified_at: std::time::SystemTime::now(),
                created_at: None,
                extension: path.extension().map(|e| e.to_string_lossy().into_owned()),
                content_hash: Some(format!("{:x}", sha2::Sha256::digest(content.as_bytes()))),
            })
        } else {
            Err(FsError::NotFound(path.display().to_string()))
        }
    }

    async fn list_dir(
        &self,
        _path: &Path,
        _recursive: bool,
    ) -> Result<Vec<FileEntry>, FsError> {
        Ok(vec![])
    }

    async fn create_dir(&self, _path: &Path) -> Result<(), FsError> {
        Ok(())
    }

    async fn delete_dir(&self, _path: &Path, _recursive: bool) -> Result<(), FsError> {
        Ok(())
    }

    async fn search_files(
        &self,
        _pattern: &str,
        _in_path: &Path,
        _max_results: usize,
    ) -> Result<Vec<SearchResult>, FsError> {
        Ok(vec![])
    }

    async fn search_content(
        &self,
        _query: &str,
        _in_path: &Path,
        _options: SearchOptions,
    ) -> Result<Vec<ContentMatch>, FsError> {
        Ok(vec![])
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
        if path.starts_with("..") {
            Err(FsError::PathEscape { path: path.display().to_string(), root: self.root_path.display().to_string() })
        } else {
            Ok(self.root_path.join(path))
        }
    }

    fn root(&self) -> &Path {
        &self.root_path
    }

    fn is_allowed(&self, path: &Path) -> bool {
        if let Ok(resolved) = self.resolve(path) {
            resolved.starts_with(&self.root_path)
        } else {
            false
        }
    }
}
