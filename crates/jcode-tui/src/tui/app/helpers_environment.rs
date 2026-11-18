//! Ambient environment, git, and clipboard probes.

use super::*;

/// Git status for `dir` (or the process working directory when `None`).
///
/// The probe gates the branch, dirty counts, and commit widgets on startup,
/// so it is latency-bound: one `rev-parse` answers "is this a work tree" and
/// "where is its root" together, then every remaining query is spawned at
/// once and collected afterwards. Sequentially these took the sum of seven
/// git processes; concurrently they take roughly the slowest one.
pub(crate) fn gather_git_info_in(dir: Option<&std::path::Path>) -> Option<GitInfo> {
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

    let mut modified = 0;
    let mut staged = 0;
    let mut untracked = 0;
    let mut all_files: Vec<crate::tui::info_widget::DirtyFile> = Vec::new();

    if let Some(status) = collect(status_child) {
        for line in status.lines() {
            if line.len() < 3 {
                continue;
            }
            let index_status = line.as_bytes()[0];
            let worktree_status = line.as_bytes()[1];
            let file_path = line[3..].to_string();

            if index_status == b'?' {
                untracked += 1;
            } else {
                if index_status != b' ' && index_status != b'?' {
                    staged += 1;
                }
                if worktree_status != b' ' && worktree_status != b'?' {
                    modified += 1;
                }
            }

            all_files.push(crate::tui::info_widget::DirtyFile::new(
                porcelain_status_letter(index_status, worktree_status),
                file_path,
            ));
        }
    }

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
    let dirty_total = all_files.len();
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
