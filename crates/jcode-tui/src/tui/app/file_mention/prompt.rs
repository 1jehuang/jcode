//! File-content loading for @file mentions, wired into the send path.

use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};

use super::search::SKIP_DIRS;
use super::types::TEXT_EXTENSIONS;

/// Maximum size of a single file before truncation.
pub(super) const MAX_FILE_SIZE: usize = 100 * 1024; // 100 KB

/// Maximum total content loaded across all @file references.
const MAX_FILE_TOTAL_BUDGET: usize = 500 * 1024; // 500 KB

/// Fast binary check: whitelisted extension → text; otherwise null-byte scan.
pub(super) fn is_likely_binary(path: &Path) -> bool {
    if let Some(ext) = path.extension().and_then(|e| e.to_str())
        && TEXT_EXTENSIONS.contains(&ext)
    {
        return false;
    }
    // Read the first 8 KB and check for null bytes.
    let mut buf = [0u8; 8192];
    if let Ok(mut file) = std::fs::File::open(path) {
        match file.read(&mut buf) {
            Ok(n) => buf[..n].contains(&0u8),
            Err(_) => false,
        }
    } else {
        false
    }
}

/// Recursively collect files from a directory, respecting SKIP_DIRS blacklist.
/// Stops after `max_files` entries.
fn collect_dir_files(
    dir: &Path,
    _cwd: &Path,
    out: &mut Vec<PathBuf>,
    seen: &mut HashSet<PathBuf>,
    max_files: usize,
) {
    if out.len() >= max_files || !dir.is_dir() {
        return;
    }
    if let Ok(rd) = std::fs::read_dir(dir) {
        for entry in rd.filter_map(|e| e.ok()) {
            let path = entry.path();
            // Skip symlinks.
            if entry.file_type().is_ok_and(|ft| ft.is_symlink()) {
                continue;
            }
            if path.is_dir() {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if SKIP_DIRS.contains(&name) {
                    continue;
                }
                collect_dir_files(&path, _cwd, out, seen, max_files);
            } else if entry.file_type().is_ok_and(|ft| ft.is_file()) && seen.insert(path.clone()) {
                // Only regular files; FIFOs/device nodes would block the
                // later content read (greptile A1).
                out.push(path);
                if out.len() >= max_files {
                    return;
                }
            }
        }
    }
}

/// Load file contents referenced by `file_chips` and prepend them to the
/// user's prompt. This is called from the send path (`submit_input`).
///
/// `file_chips` contains relative paths (as they appear in the input text).
/// Paths are resolved against the current directory. Directories are
/// recursively walked (up to 50 files per directory, respecting SKIP_DIRS).
/// The same binary detection, size truncation, and budget management applies.
/// Expand a leading `~` in a chip path to `$HOME`, leaving other paths as-is.
///
/// Chips store display strings (e.g. `~/notes/todo.txt`), so that
/// `prune_orphan_chips` can keep matching them against the input. The tilde is
/// resolved here, just before the path is used to read file contents.
pub(super) fn expand_tilde(path: &Path) -> PathBuf {
    let Some(s) = path.to_str() else {
        return path.to_path_buf();
    };
    if s == "~" {
        return std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| path.to_path_buf());
    }
    if let Some(rest) = s.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    path.to_path_buf()
}

pub(in crate::tui::app) fn build_prompt_with_files(
    input: &str,
    file_chips: &[PathBuf],
    cwd: &Path,
) -> String {
    if file_chips.is_empty() {
        return input.to_string();
    }

    let cwd = cwd.to_path_buf();

    // Expand directories into individual file paths.
    let mut expanded_files: Vec<PathBuf> = Vec::new();
    const MAX_FILES_PER_DIR: usize = 50;
    let mut seen: HashSet<PathBuf> = HashSet::new();

    for chip in file_chips {
        let abs = cwd.join(expand_tilde(chip));
        if abs.is_dir() {
            // Recursively collect files, respecting skip dirs.
            collect_dir_files(
                &abs,
                &cwd,
                &mut expanded_files,
                &mut seen,
                MAX_FILES_PER_DIR,
            );
        } else {
            if seen.insert(abs.clone()) {
                expanded_files.push(abs);
            }
        }
    }

    // Collect files synchronously (send path runs on the main thread).
    let mut file_blocks: Vec<(String, String)> = Vec::with_capacity(expanded_files.len());

    for path in &expanded_files {
        let rel_path = path
            .strip_prefix(&cwd)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| path.to_string_lossy().to_string());

        // Guard against special files before any open(2): opening a FIFO
        // with no writer blocks forever, and this loop runs on the main
        // thread at send time (greptile A1). The index paths filter these
        // out already, but a chip can survive from an older index or point
        // at a path that changed type since it was picked.
        if std::fs::metadata(path)
            .map(|m| !m.is_file())
            .unwrap_or(true)
        {
            let rp = rel_path.clone();
            file_blocks.push((rp, format!("[skipped: not a regular file {}]", rel_path)));
            continue;
        }

        if is_likely_binary(path) {
            let rp = rel_path.clone();
            file_blocks.push((rp, format!("[skipped: binary file {}]", rel_path)));
            continue;
        }

        // Check the size from metadata *before* reading: a chip pointing at a
        // huge file must not be fully loaded into memory just to be rejected.
        let file_size = std::fs::metadata(path).map(|m| m.len()).ok();
        if file_size.is_some_and(|size| size > MAX_FILE_SIZE as u64) {
            let size = file_size.unwrap_or(0);
            let preview = read_prefix_lines(path, 200).unwrap_or_default();
            let block = if preview.trim().is_empty() {
                format!("[file too large: {} bytes, showing first 200 lines]", size)
            } else {
                format!(
                    "{}\n\n[... file too large: {} bytes, showing first 200 lines]",
                    preview, size
                )
            };
            file_blocks.push((rel_path, block));
            continue;
        }

        match read_to_string_bounded(path, MAX_FILE_SIZE) {
            Ok(content) => {
                file_blocks.push((rel_path, content));
            }
            Err(e) => {
                let rp = rel_path.clone();
                file_blocks.push((rp, format!("[read failed: {} → {}]", rel_path, e)));
            }
        }
    }

    // Context budget: cumulative cap.
    let mut context = String::new();
    let mut total = 0usize;
    for (path, content) in &file_blocks {
        if total + content.len() > MAX_FILE_TOTAL_BUDGET {
            context.push_str(&format!(
                "\n--- {} ---\n[skipped: context budget exhausted ({} KB total)]\n",
                path,
                MAX_FILE_TOTAL_BUDGET / 1024,
            ));
        } else {
            total += content.len();
            context.push_str(&format!("\n--- {} ---\n{}\n", path, content));
        }
    }

    format!("{}{}", input, context)
}

/// Read at most `limit` bytes of a file as a string.
///
/// Metadata can race with concurrent writes, so a file that grew past the
/// limit between the stat and the read is still capped here instead of
/// ballooning memory.
///
/// UTF-8 validation matches `read_to_string` with one carve-out: when the
/// read stopped exactly at the byte limit and the *only* problem is a
/// multi-byte character sliced in half at that boundary, the file itself is
/// valid and the truncated prefix is kept. Any invalid byte before the
/// limit is genuine corruption and returns `InvalidData` so the caller can
/// surface a read-failure marker instead of silently embedding a partial
/// file.
pub(super) fn read_to_string_bounded(path: &Path, limit: usize) -> std::io::Result<String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut buf = Vec::with_capacity(limit.min(64 * 1024));
    (&mut file).take(limit as u64).read_to_end(&mut buf)?;
    let read_len = buf.len();
    match String::from_utf8(buf) {
        Ok(s) => Ok(s),
        Err(e) => {
            let valid_up_to = e.utf8_error().valid_up_to();
            match e.utf8_error().error_len() {
                // No error_len means the input ended mid-character: the byte
                // limit sliced a multi-byte char exactly at the cut.
                None if read_len == limit => {
                    let bytes = e.into_bytes();
                    let s = String::from_utf8_lossy(&bytes[..valid_up_to]).into_owned();
                    Ok(s)
                }
                _ => Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "stream did not contain valid UTF-8",
                )),
            }
        }
    }
}

/// Read the first `max_lines` lines of a file (bounded by MAX_FILE_SIZE).
fn read_prefix_lines(path: &Path, max_lines: usize) -> std::io::Result<String> {
    let content = read_to_string_bounded(path, MAX_FILE_SIZE)?;
    Ok(content
        .lines()
        .take(max_lines)
        .collect::<Vec<_>>()
        .join("\n"))
}
