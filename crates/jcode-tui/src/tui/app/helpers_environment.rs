//! Ambient environment, git, and clipboard probes.

use super::*;

/// Work bounds for the startup `git status` probe.
///
/// `git status --porcelain --untracked-files=all` in a pathological work tree
/// (an unignored build tree, a checked-in dependency cache) can emit gigabytes
/// and take seconds. This probe runs on the startup path, so it stops reading
/// once any bound is reached and kills the child. For ordinary repositories
/// (fewer than [`GitStatusLimits::max_detailed`] dirty paths) nothing changes:
/// counts, newest-first ordering, and line totals are exact. Between
/// `max_detailed` and `max_entries` the counts stay exact while per-file
/// detail covers only the retained window.
///
/// Once truncated, the counts, `dirty_total`, and the line totals are lower
/// bounds rather than exact totals. They are still consistent with each other
/// and with the retained files, so the widget never reports a count above what
/// it has seen.
#[derive(Debug, Clone, Copy)]
struct GitStatusLimits {
    /// Bytes of porcelain output read before giving up. Keeps the lossy
    /// UTF-8 buffer and the child's write time bounded.
    max_bytes: usize,
    /// Porcelain entries counted (and therefore the maximum `dirty_total`).
    max_entries: usize,
    /// Entries that get a retained path, an mtime probe, and an untracked
    /// line count. The Changes widget only shows 10, but retaining a larger
    /// window keeps newest-first ordering useful on busy real repos.
    max_detailed: usize,
}

impl Default for GitStatusLimits {
    fn default() -> Self {
        Self {
            // ~4 MiB is tens of thousands of porcelain lines for normal path
            // lengths, far beyond the handful a widget can use.
            max_bytes: 4 * 1024 * 1024,
            max_entries: 2048,
            max_detailed: 256,
        }
    }
}

#[derive(Debug, Default)]
struct GitStatusScan {
    modified: usize,
    staged: usize,
    untracked: usize,
    /// Entries counted, including ones beyond `max_detailed`.
    total: usize,
    /// The first `max_detailed` entries, in git's output order.
    files: Vec<crate::tui::info_widget::DirtyFile>,
    /// True when a limit stopped the scan before git's output ended.
    truncated: bool,
}

/// Count porcelain entries with bounded work.
///
/// Parsing mirrors the previous whole-output loop exactly (including skipping
/// short lines and treating `??` as untracked), but stops at the limits
/// instead of materializing every path. Kept as a free function so the bounds
/// can be exercised without building a pathological repository.
fn scan_git_status(mut reader: impl std::io::BufRead, limits: GitStatusLimits) -> GitStatusScan {
    let mut scan = GitStatusScan::default();
    let mut line = Vec::new();
    let mut bytes = 0usize;
    loop {
        if scan.total >= limits.max_entries || bytes >= limits.max_bytes {
            // Distinguish "output ended exactly at the cap" from a real
            // truncation without consuming the next entry. At EOF this is
            // empty; otherwise the child still has output pending.
            scan.truncated = reader.fill_buf().map(|buf| !buf.is_empty()).unwrap_or(true);
            break;
        }
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) => break,
            Ok(read) => bytes += read,
            Err(_) => break,
        }
        if line.len() < 3 {
            continue;
        }
        let index_status = line[0];
        let worktree_status = line[1];
        let raw_path = String::from_utf8_lossy(&line[3..]);
        let file_path = raw_path.trim_end_matches(['\n', '\r']).to_string();

        if index_status == b'?' {
            scan.untracked += 1;
        } else {
            if index_status != b' ' && index_status != b'?' {
                scan.staged += 1;
            }
            if worktree_status != b' ' && worktree_status != b'?' {
                scan.modified += 1;
            }
        }
        scan.total += 1;
        if scan.files.len() < limits.max_detailed {
            scan.files.push(crate::tui::info_widget::DirtyFile::new(
                porcelain_status_letter(index_status, worktree_status),
                file_path,
            ));
        }
    }
    scan
}

/// Git status for `dir` (or the process working directory when `None`).
///
/// The probe gates the branch, dirty counts, and commit widgets on startup,
/// so it is latency-bound: one `rev-parse` answers "is this a work tree" and
/// "where is its root" together, then every remaining query is spawned at
/// once and collected afterwards. Sequentially these took the sum of seven
/// git processes; concurrently they take roughly the slowest one.
pub(crate) fn gather_git_info_in(dir: Option<&std::path::Path>) -> Option<GitInfo> {
    gather_git_info_with_limits(dir, GitStatusLimits::default())
}

fn gather_git_info_with_limits(
    dir: Option<&std::path::Path>,
    limits: GitStatusLimits,
) -> Option<GitInfo> {
    let git = || {
        let mut cmd = std::process::Command::new("git");
        if let Some(dir) = dir {
            cmd.current_dir(dir);
        }
        // A background probe must never contend with the user's own git
        // commands for `index.lock` (status would otherwise opportunistically
        // refresh and rewrite the index).
        cmd.env("GIT_OPTIONAL_LOCKS", "0");
        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        cmd
    };
    let spawn = |args: &[&str]| match git().args(args).spawn() {
        Ok(child) => Some(child),
        Err(error) => {
            crate::logging::debug(&format!("Cannot start background git probe: {error}"));
            None
        }
    };
    let collect = |child: Option<std::process::Child>| {
        child
            .and_then(|child| match child.wait_with_output() {
                Ok(output) => Some(output),
                Err(error) => {
                    crate::logging::debug(&format!("Cannot collect background git probe: {error}"));
                    None
                }
            })
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
    };

    let probe = collect(spawn(&[
        "rev-parse",
        "--is-inside-work-tree",
        "--show-toplevel",
    ]))?;
    let mut probe_lines = probe.lines();
    if probe_lines.next().map(str::trim) != Some("true") {
        return None;
    }
    let repo_root = probe_lines
        .next()
        .map(str::trim)
        .filter(|root| !root.is_empty())
        .map(std::path::PathBuf::from);

    let branch_child = spawn(&["branch", "--show-current"]);
    let status_child = spawn(&["status", "--porcelain", "--untracked-files=all"]);
    let numstat_child = spawn(&["diff", "--numstat", "HEAD"]);
    let ahead_behind_child = spawn(&["rev-list", "--left-right", "--count", "HEAD...@{upstream}"]);
    let log_child = spawn(&[
        "log",
        "-n",
        "8",
        "--shortstat",
        "--format=%x1e%h%x1f%ct%x1f%s",
    ]);

    let branch = collect(branch_child)
        .map(|b| b.trim().to_string())
        .filter(|b| !b.is_empty())
        .unwrap_or_else(|| "HEAD".to_string());

    // Stream the status instead of buffering it: `collect` above would hold
    // every byte of a pathological work tree in memory before parsing a line.
    let mut scan = match status_child {
        Some(mut child) => {
            let scan = child
                .stdout
                .take()
                .map(|stdout| scan_git_status(std::io::BufReader::new(stdout), limits))
                .unwrap_or_default();
            // If we stopped early, stop git too rather than leaving it to
            // produce output nobody reads.
            if scan.truncated {
                let _ = child.kill();
            }
            if let Err(error) = child.wait() {
                crate::logging::debug(&format!("Failed to reap git status probe: {error}"));
            }
            scan
        }
        None => GitStatusScan::default(),
    };
    if scan.truncated {
        // Counts and line totals below are lower bounds from here on.
        crate::logging::debug(&format!(
            "git status probe truncated after {} entries; dirty counts are lower bounds",
            scan.total
        ));
    }
    let (modified, staged, untracked) = (scan.modified, scan.staged, scan.untracked);
    let mut all_files = std::mem::take(&mut scan.files);

    // Line counts: tracked files from one numstat against HEAD (staged plus
    // unstaged), untracked files by counting their lines.
    let numstat = collect(numstat_child)
        .map(|text| parse_numstat(&text))
        .unwrap_or_default();
    let mut added_total = 0usize;
    let mut removed_total = 0usize;
    let mut line_cache = GIT_LINE_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    line_cache.begin_scan();
    for file in &mut all_files {
        let key = file
            .path
            .rsplit(" -> ")
            .next()
            .unwrap_or(&file.path)
            .trim_matches('"');
        let abs = repo_root.as_ref().map(|root| root.join(key));
        let metadata = abs
            .as_deref()
            .and_then(|p| std::fs::symlink_metadata(p).ok());
        if file.status == '?' {
            file.added = abs
                .as_deref()
                .zip(metadata.as_ref())
                .and_then(|(path, meta)| line_cache.count(path, meta));
            file.removed = file.added.map(|_| 0);
        } else if let Some(&(a, r)) = numstat.get(key) {
            file.added = a;
            file.removed = r;
        }
        added_total += file.added.unwrap_or(0);
        removed_total += file.removed.unwrap_or(0);
        file.modified_at = metadata.and_then(|m| m.modified().ok());
    }
    line_cache.finish_scan();
    drop(line_cache);
    // Newest first, so the file being worked on stays visible under the cap.
    // Deleted files have no mtime and sort last.
    all_files.sort_by(|a, b| b.modified_at.cmp(&a.modified_at));
    // Beyond `max_detailed` entries we only counted, so the newest-first order
    // and per-file line totals describe the retained window (a lower bound in
    // pathological trees), never a fabricated full-repo total.
    let dirty_total = scan.total;
    all_files.truncate(10);
    let dirty_files = all_files;

    let (ahead, behind) = collect(ahead_behind_child)
        .and_then(|text| {
            let parts: Vec<&str> = text.trim().split('\t').collect();
            (parts.len() == 2).then(|| {
                (
                    parts[0].parse::<usize>().unwrap_or(0),
                    parts[1].parse::<usize>().unwrap_or(0),
                )
            })
        })
        .unwrap_or((0, 0));

    let recent_commits = collect(log_child)
        .map(|text| parse_recent_commits(&text, ahead))
        .unwrap_or_default();

    Some(GitInfo {
        branch,
        modified,
        staged,
        untracked,
        ahead,
        behind,
        dirty_files,
        dirty_total,
        added_total,
        removed_total,
        repo_root,
        recent_commits,
    })
}

/// Try to get an image from the system clipboard.
///
/// Returns `Some((media_type, base64_data))` if an image is available.
/// Uses `wl-paste` on Wayland, `osascript` on macOS, falls back to `arboard::get_image()`.
pub(in super::super) fn clipboard_image() -> Option<(String, String)> {
    use base64::Engine;

    // Try wl-paste first (native Wayland - better image format support)
    if std::env::var("WAYLAND_DISPLAY").is_ok()
        && let Ok(output) = std::process::Command::new("wl-paste")
            .arg("--list-types")
            .output()
    {
        let types = String::from_utf8_lossy(&output.stdout);
        crate::logging::info(&format!(
            "clipboard_image: wl-paste types: {:?}",
            types.trim()
        ));
        let (mime, wl_type) = if types.lines().any(|t| t.trim() == "image/png") {
            ("image/png", "image/png")
        } else if types.lines().any(|t| t.trim() == "image/jpeg") {
            ("image/jpeg", "image/jpeg")
        } else if types.lines().any(|t| t.trim() == "image/webp") {
            ("image/webp", "image/webp")
        } else if types.lines().any(|t| t.trim() == "image/gif") {
            ("image/gif", "image/gif")
        } else {
            ("", "")
        };

        if !mime.is_empty()
            && let Ok(img_output) = std::process::Command::new("wl-paste")
                .args(["--type", wl_type, "--no-newline"])
                .output()
            && img_output.status.success()
            && !img_output.stdout.is_empty()
        {
            let b64 = base64::engine::general_purpose::STANDARD.encode(&img_output.stdout);
            return Some((mime.to_string(), b64));
        }

        // Fallback: check text/html for <img> tags (Discord copies HTML with image URLs)
        if types.lines().any(|t| t.trim() == "text/html")
            && let Ok(html_output) = std::process::Command::new("wl-paste")
                .args(["--type", "text/html"])
                .output()
            && html_output.status.success()
            && !html_output.stdout.is_empty()
        {
            let html = String::from_utf8_lossy(&html_output.stdout);
            crate::logging::info(&format!(
                "clipboard_image: checking HTML for img tags ({} bytes)",
                html.len()
            ));
            if let Some(url) = extract_image_url(&html) {
                crate::logging::info(&format!(
                    "clipboard_image: found image URL in HTML: {}",
                    jcode_core::util::truncate_str(&url, 80)
                ));
                if let Some(result) = download_image_url(&url) {
                    return Some(result);
                }
            }
        }
    }

    // macOS: use osascript to check clipboard for images and save as PNG via temp file
    #[cfg(target_os = "macos")]
    {
        let temp_path = std::env::temp_dir().join("jcode_clipboard.png");
        let script = format!(
            r#"use framework \"AppKit\"
            set pb to current application's NSPasteboard's generalPasteboard()
            set imgClasses to current application's NSArray's arrayWithObject:(current application's NSImage)
            if (pb's canReadObjectForClasses:imgClasses options:(missing value)) then
                set imgList to pb's readObjectsForClasses:imgClasses options:(missing value)
                set img to item 1 of imgList
                set tiffData to img's TIFFRepresentation()
                set bitmapRep to current application's NSBitmapImageRep's imageRepWithData:tiffData
                set pngData to bitmapRep's representationUsingType:(current application's NSBitmapImageFileTypePNG) properties:(missing value)
                pngData's writeToFile:\"{}\" atomically:true
                return \"ok\"
            else
                return \"none\"
            end if"#,
            temp_path.to_string_lossy()
        );
        if let Ok(output) = std::process::Command::new("osascript")
            .args(["-l", "AppleScript", "-e", &script])
            .output()
        {
            let result = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if result == "ok"
                && let Ok(data) = std::fs::read(&temp_path)
            {
                let _ = std::fs::remove_file(&temp_path);
                if !data.is_empty() {
                    let b64 = base64::engine::general_purpose::STANDARD.encode(&data);
                    return Some(("image/png".to_string(), b64));
                }
            }
        }
    }

    // Fallback: arboard (works on X11/XWayland and macOS via NSPasteboard)
    if let Ok(mut clipboard) = arboard::Clipboard::new()
        && let Ok(img) = clipboard.get_image()
        && let Some(png_data) = encode_rgba_as_png(img.width, img.height, &img.bytes)
    {
        let b64 = base64::engine::general_purpose::STANDARD.encode(&png_data);
        return Some(("image/png".to_string(), b64));
    }

    None
}

pub(super) fn gather_ambient_info_inner(ambient_enabled: bool) -> Option<AmbientWidgetData> {
    let state = crate::ambient::AmbientState::load().unwrap_or_default();
    let manager = crate::ambient::AmbientManager::new().ok();
    let queue_items: Vec<_> = manager
        .as_ref()
        .map(|m| m.queue().items().to_vec())
        .unwrap_or_default();
    let queue_count = queue_items.len();
    let next_queue_item = queue_items.iter().min_by_key(|item| item.scheduled_for);
    let reminder_items: Vec<_> = queue_items
        .iter()
        .filter(|item| item.target.is_direct_delivery())
        .collect();
    let reminder_count = reminder_items.len();
    let next_reminder_item = reminder_items
        .iter()
        .min_by_key(|item| item.scheduled_for)
        .copied();

    if !ambient_enabled && reminder_count == 0 {
        return None;
    }

    let last_run_ago = state.last_run.map(|t| {
        let ago = chrono::Utc::now() - t;
        if ago.num_hours() > 0 {
            format!("{}h ago", ago.num_hours())
        } else {
            format!("{}m ago", ago.num_minutes().max(0))
        }
    });
    let next_wake = match &state.status {
        crate::ambient::AmbientStatus::Scheduled { next_wake } => {
            Some(format_countdown_until(*next_wake))
        }
        _ => None,
    };

    let next_queue_preview = next_queue_item.map(|item| {
        item.task_description
            .as_deref()
            .unwrap_or(&item.context)
            .to_string()
    });
    let next_reminder_preview = next_reminder_item.map(|item| {
        item.task_description
            .as_deref()
            .unwrap_or(&item.context)
            .to_string()
    });

    Some(AmbientWidgetData {
        show_widget: ambient_enabled || reminder_count > 1,
        status: state.status,
        queue_count,
        next_queue_preview,
        reminder_count,
        next_reminder_preview,
        last_run_ago,
        last_summary: state.last_summary,
        next_wake,
        next_reminder_wake: next_reminder_item
            .map(|item| format_countdown_until(item.scheduled_for)),
        budget_percent: None,
    })
}

#[cfg(test)]
mod git_status_bounds_tests {
    use super::{GitStatusLimits, gather_git_info_with_limits, scan_git_status};

    fn limits(max_bytes: usize, max_entries: usize, max_detailed: usize) -> GitStatusLimits {
        GitStatusLimits {
            max_bytes,
            max_entries,
            max_detailed,
        }
    }

    #[test]
    fn status_scan_stops_at_entry_cap() {
        let mut input = String::new();
        for index in 0..50 {
            input.push_str(&format!("?? file{index:03}.txt\n"));
        }
        let scan = scan_git_status(std::io::Cursor::new(input), limits(usize::MAX, 7, 3));
        assert!(scan.truncated);
        assert_eq!(
            (scan.untracked, scan.total, scan.files.len()),
            (7, 7, 3),
            "entry and detailed caps bound the scan"
        );
    }

    #[test]
    fn status_scan_stops_at_byte_cap() {
        let mut input = String::new();
        for index in 0..50 {
            input.push_str(&format!(" M file{index:03}.txt\n"));
        }
        let scan = scan_git_status(std::io::Cursor::new(input), limits(64, 1000, 100));
        assert!(scan.truncated);
        assert!(
            scan.total > 0 && scan.total < 50,
            "byte cap stopped the scan early: {}",
            scan.total
        );
        assert_eq!(scan.modified, scan.total);
    }

    #[test]
    fn status_scan_does_not_flag_truncation_at_exact_eof() {
        let scan = scan_git_status(
            std::io::Cursor::new("?? a\n?? b\n"),
            limits(usize::MAX, 2, 10),
        );
        assert_eq!((scan.total, scan.files.len()), (2, 2));
        assert!(
            !scan.truncated,
            "a cap reached exactly at EOF is not truncation"
        );
    }

    #[test]
    fn many_untracked_files_keep_exact_counts_and_bounded_retention() {
        let dir = tempfile::tempdir().unwrap();
        let status = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(dir.path())
            .status()
            .unwrap();
        assert!(status.success());
        for index in 0..20 {
            std::fs::write(
                dir.path().join(format!("untracked-{index:02}.txt")),
                "one\ntwo\n",
            )
            .unwrap();
        }

        let info = gather_git_info_with_limits(Some(dir.path()), GitStatusLimits::default())
            .expect("git probe");
        assert_eq!((info.untracked, info.dirty_total), (20, 20));
        assert_eq!(info.dirty_files.len(), 10, "display keeps the newest ten");

        let bounded = gather_git_info_with_limits(Some(dir.path()), limits(usize::MAX, 7, 3))
            .expect("bounded git probe");
        assert_eq!((bounded.untracked, bounded.dirty_total), (7, 7));
        assert!(
            bounded.dirty_files.len() <= 3,
            "detailed retention respects its cap: {}",
            bounded.dirty_files.len()
        );
        assert!(bounded.dirty_files.len() <= bounded.dirty_total);
    }
}
