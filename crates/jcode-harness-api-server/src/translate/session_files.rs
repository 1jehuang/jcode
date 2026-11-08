use super::*;

impl BridgeState {
    pub(super) fn app_config_dir() -> Option<std::path::PathBuf> {
        if let Some(home) = std::env::var_os("JCODE_HOME") {
            return Some(std::path::Path::new(&home).join("config/jcode"));
        }
        #[cfg(target_os = "macos")]
        return Some(
            std::path::Path::new(&std::env::var_os("HOME")?)
                .join("Library/Application Support/jcode"),
        );
        #[cfg(target_os = "windows")]
        return Some(std::path::Path::new(&std::env::var_os("APPDATA")?).join("jcode"));
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        Some(
            std::env::var_os("XDG_CONFIG_HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| {
                    std::path::Path::new(&std::env::var_os("HOME").unwrap_or_default())
                        .join(".config")
                })
                .join("jcode"),
        )
    }

    pub(super) fn session_root(
        session_id: &str,
    ) -> Result<std::path::PathBuf, (ErrorCode, String)> {
        let root = Self::resolve_working_dir(session_id).ok_or((
            ErrorCode::UnknownSession,
            "session has no working directory".into(),
        ))?;
        std::fs::canonicalize(root).map_err(|error| {
            (
                ErrorCode::InvalidRequest,
                format!("working directory is unavailable: {error}"),
            )
        })
    }

    pub(super) fn safe_session_path(
        session_id: &str,
        relative: &str,
    ) -> Result<std::path::PathBuf, (ErrorCode, String)> {
        if relative.is_empty() || std::path::Path::new(relative).is_absolute() {
            return Err((
                ErrorCode::InvalidRequest,
                "path must be a non-empty relative path".into(),
            ));
        }
        if std::path::Path::new(relative)
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
        {
            return Err((
                ErrorCode::InvalidRequest,
                "path may not contain `..`".into(),
            ));
        }
        let root = Self::session_root(session_id)?;
        let candidate = root.join(relative);
        if candidate.exists() {
            let canonical = std::fs::canonicalize(&candidate)
                .map_err(|error| (ErrorCode::InvalidRequest, error.to_string()))?;
            if !canonical.starts_with(&root) {
                return Err((
                    ErrorCode::InvalidRequest,
                    "path escapes the session working directory".into(),
                ));
            }
            return Ok(canonical);
        }
        // A missing final path can still be safely statted when its nearest
        // existing ancestor remains under the canonical root.
        let mut ancestor = candidate.as_path();
        while !ancestor.exists() {
            ancestor = ancestor
                .parent()
                .ok_or((ErrorCode::InvalidRequest, "invalid path".into()))?;
        }
        let canonical = std::fs::canonicalize(ancestor)
            .map_err(|error| (ErrorCode::InvalidRequest, error.to_string()))?;
        if !canonical.starts_with(&root) {
            return Err((
                ErrorCode::InvalidRequest,
                "path escapes the session working directory".into(),
            ));
        }
        Ok(candidate)
    }

    pub(super) fn read_session_file(
        session_id: &str,
        relative: &str,
        max: u64,
    ) -> Result<(String, u64, bool), (ErrorCode, String)> {
        let path = Self::safe_session_path(session_id, relative)?;
        let metadata = path
            .metadata()
            .map_err(|error| (ErrorCode::InvalidRequest, error.to_string()))?;
        if !metadata.is_file() {
            return Err((
                ErrorCode::InvalidRequest,
                "path is not a regular file".into(),
            ));
        }
        let size = metadata.len();
        let file = std::fs::File::open(path)
            .map_err(|error| (ErrorCode::InvalidRequest, error.to_string()))?;
        let mut bytes = Vec::new();
        file.take(max.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|error| (ErrorCode::Internal, error.to_string()))?;
        let truncated = bytes.len() as u64 > max;
        bytes.truncate(max as usize);
        let content = match std::str::from_utf8(&bytes) {
            Ok(content) => content.to_string(),
            Err(error) if truncated && error.error_len().is_none() => {
                String::from_utf8(bytes[..error.valid_up_to()].to_vec())
                    .expect("valid UTF-8 prefix")
            }
            Err(_) => {
                return Err((ErrorCode::InvalidRequest, "file is not UTF-8 text".into()));
            }
        };
        Ok((content, size, truncated))
    }

    pub(super) fn walk_files(root: &std::path::Path, limit: usize) -> Vec<std::path::PathBuf> {
        let mut files = Vec::new();
        let mut pending = vec![root.to_path_buf()];
        let mut scanned_entries = 0usize;
        while let Some(dir) = pending.pop() {
            let Ok(entries) = std::fs::read_dir(dir) else {
                continue;
            };
            let mut entries: Vec<_> = entries.flatten().collect();
            entries.sort_by_key(|entry| entry.file_name());
            for entry in entries {
                scanned_entries += 1;
                if scanned_entries > MAX_WALK_ENTRIES {
                    return files;
                }
                let name = entry.file_name();
                if matches!(
                    name.to_str(),
                    Some(".git" | "node_modules" | "target" | ".next")
                ) {
                    continue;
                }
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                if kind.is_symlink() {
                    continue;
                }
                if kind.is_dir() {
                    pending.push(entry.path());
                }
                if kind.is_file() {
                    files.push(entry.path());
                    if files.len() >= limit {
                        return files;
                    }
                }
            }
        }
        files
    }

    pub(super) fn find_session_files(
        session_id: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<String>, (ErrorCode, String)> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let root = Self::session_root(session_id)?;
        let needle = query.to_ascii_lowercase();
        // Scan a bounded project-wide set before applying the name filter.
        // Stopping at `limit * 10` made a query incorrectly return no results
        // whenever the matching directory happened to sort later in a large
        // repository.
        Ok(Self::walk_files(&root, MAX_WALK_FILES)
            .into_iter()
            .filter_map(|path| {
                let relative = path
                    .strip_prefix(&root)
                    .ok()?
                    .to_string_lossy()
                    .replace('\\', "/");
                relative
                    .to_ascii_lowercase()
                    .contains(&needle)
                    .then_some(relative)
            })
            .take(limit)
            .collect())
    }

    pub(super) fn search_session_text(
        session_id: &str,
        query: &str,
        under: Option<&str>,
        limit: usize,
    ) -> Result<Vec<TextMatch>, (ErrorCode, String)> {
        if query.is_empty() {
            return Err((ErrorCode::InvalidRequest, "query must not be empty".into()));
        }
        if limit == 0 {
            return Ok(Vec::new());
        }
        let root = Self::session_root(session_id)?;
        let search_root = match under {
            Some(relative) => Self::safe_session_path(session_id, relative)?,
            None => root.clone(),
        };
        let files = if search_root.is_file() {
            vec![search_root]
        } else {
            Self::walk_files(&search_root, MAX_WALK_FILES)
        };
        let mut matches = Vec::new();
        let mut remaining_bytes = MAX_SEARCH_BYTES;
        for path in files {
            let Some(size) = path.metadata().ok().map(|meta| meta.len()) else {
                continue;
            };
            if size > DEFAULT_FILE_BYTES || size > remaining_bytes {
                continue;
            }
            remaining_bytes -= size;
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            for (line_index, line) in text.lines().enumerate() {
                for (byte_column, _) in line.match_indices(query) {
                    matches.push(TextMatch {
                        path: path
                            .strip_prefix(&root)
                            .unwrap_or(&path)
                            .to_string_lossy()
                            .replace('\\', "/"),
                        line: line_index as u32 + 1,
                        column: line[..byte_column].chars().count() as u32 + 1,
                        preview: line.chars().take(500).collect(),
                    });
                    if matches.len() >= limit {
                        return Ok(matches);
                    }
                }
            }
        }
        Ok(matches)
    }

    pub(super) fn session_file_status(session_id: &str, relative: &str) -> SessionFileStatusResult {
        let path = Self::safe_session_path(session_id, relative)?;
        let Ok(meta) = path.metadata() else {
            return Ok((false, "missing".into(), None, None));
        };
        let kind = if meta.is_file() {
            "file"
        } else if meta.is_dir() {
            "directory"
        } else {
            "other"
        };
        let modified = meta
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64);
        Ok((
            true,
            kind.into(),
            meta.is_file().then_some(meta.len()),
            modified,
        ))
    }
}
