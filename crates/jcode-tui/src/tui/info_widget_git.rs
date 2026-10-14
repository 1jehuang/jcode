//! Changes widget: the detail layer behind the status line's git segment.
//!
//! The overscroll status line already owns the branch and the dirty/ahead
//! counts (`main ~3 ?2 ↑1`). This widget never repeats those. It answers the
//! follow-up question instead: *which* files are dirty.

use super::text::truncate_smart;
use super::{DirtyFile, GitInfo, InfoWidgetData};
use crate::tui::color_support::rgb;
use ratatui::prelude::*;

/// Maximum file rows before collapsing the rest into a `+N more` row.
pub(super) const CHANGES_MAX_FILES: usize = 5;

/// Paths an edit-style tool call wrote to, as given in its input (absolute or
/// relative to the session working directory). Non-edit tools yield nothing.
pub(crate) fn edited_paths_from_tool_call(name: &str, input: &serde_json::Value) -> Vec<String> {
    if !crate::tui::ui::tools_ui::is_edit_tool_name(name) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for key in ["file_path", "path"] {
        if let Some(p) = input.get(key).and_then(|v| v.as_str()) {
            out.push(p.to_string());
        }
    }
    if let Some(text) = input
        .get("patch_text")
        .or_else(|| input.get("patch"))
        .and_then(|v| v.as_str())
    {
        for line in text.lines() {
            let t = line.trim();
            let codex = t
                .strip_prefix("*** Update File: ")
                .or_else(|| t.strip_prefix("*** Add File: "))
                .or_else(|| t.strip_prefix("*** Move to: "));
            let unified = line
                .strip_prefix("+++ ")
                .map(|r| r.split('\t').next().unwrap_or(r))
                .filter(|r| *r != "/dev/null")
                .map(|r| r.strip_prefix("b/").unwrap_or(r));
            if let Some(p) = codex.or(unified) {
                out.push(p.trim().to_string());
            }
        }
    }
    out
}

/// Resolve an edited path to an absolute, lexically normalized path.
/// Relative paths are taken relative to the session working directory.
pub(crate) fn resolve_edited_path(path: &str, working_dir: Option<&str>) -> std::path::PathBuf {
    let p = std::path::Path::new(path);
    let joined = if p.is_absolute() {
        p.to_path_buf()
    } else if let Some(wd) = working_dir {
        std::path::Path::new(wd).join(p)
    } else {
        p.to_path_buf()
    };
    let mut out = std::path::PathBuf::new();
    for c in joined.components() {
        match c {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// Whether a repo-relative dirty path is one the agent edited this session.
pub(crate) fn is_agent_edited(
    repo_root: Option<&std::path::Path>,
    repo_path: &str,
    edited: &std::collections::HashSet<std::path::PathBuf>,
) -> bool {
    let Some(root) = repo_root else {
        return false;
    };
    let rel = repo_path.rsplit(" -> ").next().unwrap_or(repo_path).trim_matches('"');
    edited.contains(&root.join(rel.trim_end_matches('/')))
}

/// Color of the agent-edit marker, shared by the rows and the legend.
pub(super) const AGENT_DOT_COLOR: (u8, u8, u8) = (186, 139, 255);

/// Border legend explaining the agent marker, shown only when at least one
/// visible row carries it so it never labels something absent.
pub(super) fn changes_legend(data: &InfoWidgetData, inner_height: u16) -> Option<Line<'static>> {
    let info = data.git_info.as_ref()?;
    let total = info.dirty_total.max(info.dirty_files.len());
    let mut rows = (inner_height as usize).min(CHANGES_MAX_FILES);
    if total > rows && rows > 0 {
        rows -= 1;
    }
    let root = info.repo_root.as_deref();
    let any = info
        .dirty_files
        .iter()
        .take(rows)
        .any(|f| is_agent_edited(root, &f.path, &data.agent_edited));
    any.then(|| {
        let (r, g, b) = AGENT_DOT_COLOR;
        Line::from(vec![
            Span::raw(" "),
            Span::styled("●", Style::default().fg(rgb(r, g, b))),
            Span::styled(" edited by agent ", Style::default().fg(rgb(130, 130, 145))),
        ])
        .right_aligned()
    })
}

/// Whether the Changes widget has anything to show. A clean tree (or one that
/// is only ahead/behind) has no file detail, and the status line already
/// covers ahead/behind.
pub(super) fn changes_has_data(info: &GitInfo) -> bool {
    !info.dirty_files.is_empty()
}

/// Content rows the Changes widget renders, mirroring [`render_git_widget`].
pub(super) fn changes_height(info: &GitInfo) -> u16 {
    if !changes_has_data(info) {
        return 0;
    }
    // Up to CHANGES_MAX_FILES rows. When files overflow, the last of those
    // rows becomes `+N more`, so the height never exceeds the cap.
    let total = info.dirty_total.max(info.dirty_files.len());
    let shown = info.dirty_files.len().min(CHANGES_MAX_FILES);
    if total > shown {
        CHANGES_MAX_FILES.min(shown + 1) as u16
    } else {
        shown as u16
    }
}

pub(super) fn render_git_widget(data: &InfoWidgetData, inner: Rect) -> Vec<Line<'static>> {
    let Some(info) = &data.git_info else {
        return Vec::new();
    };
    if !changes_has_data(info) {
        return Vec::new();
    }

    let w = inner.width as usize;
    let total = info.dirty_total.max(info.dirty_files.len());
    let mut max_files = (inner.height as usize).min(CHANGES_MAX_FILES);
    if total > max_files && max_files > 0 {
        // Reserve the last row for the overflow count.
        max_files -= 1;
    }

    let shown: Vec<&DirtyFile> = info.dirty_files.iter().take(max_files).collect();
    // One shared column width for `+N −M`, so the counts line up.
    let count_w = shown
        .iter()
        .map(|f| line_counts(f).map_or(0, |(a, r)| a.chars().count() + 1 + r.chars().count()))
        .max()
        .unwrap_or(0);
    let root = info.repo_root.as_deref();
    let mut lines: Vec<Line<'static>> = shown
        .iter()
        .map(|file| {
            let agent = is_agent_edited(root, &file.path, &data.agent_edited);
            changes_file_line(file, agent, count_w, w)
        })
        .collect();

    let hidden = total.saturating_sub(lines.len());
    if hidden > 0 {
        let dim = Style::default().fg(rgb(100, 100, 115));
        let mut spans = vec![Span::styled(format!("  +{hidden} more"), dim)];
        if info.added_total + info.removed_total > 0 {
            let totals = format!(
                "+{} −{} all",
                info.added_total, info.removed_total
            );
            let used = 2 + format!("+{hidden} more").chars().count();
            let pad = w.saturating_sub(used + totals.chars().count());
            if pad >= 1 {
                spans.push(Span::raw(" ".repeat(pad)));
                spans.push(Span::styled(
                    format!("+{}", info.added_total),
                    Style::default().fg(rgb(90, 170, 100)),
                ));
                spans.push(Span::styled(
                    format!(" −{}", info.removed_total),
                    Style::default().fg(rgb(200, 100, 95)),
                ));
                spans.push(Span::styled(" all", dim));
            }
        }
        lines.push(Line::from(spans));
    }
    lines
}

/// `(+added, −removed)` labels, or `None` when counts are unknown (binary).
fn line_counts(file: &DirtyFile) -> Option<(String, String)> {
    let added = file.added?;
    let removed = file.removed.unwrap_or(0);
    Some((format!("+{added}"), format!("−{removed}")))
}

fn changes_file_line(file: &DirtyFile, agent: bool, count_w: usize, width: usize) -> Line<'static> {
    let (letter_color, path_color) = match file.status {
        'A' => (rgb(100, 200, 100), rgb(170, 190, 170)),
        'D' => (rgb(255, 120, 110), rgb(150, 130, 130)),
        'R' => (rgb(140, 180, 255), rgb(160, 170, 190)),
        'U' => (rgb(255, 90, 90), rgb(230, 170, 160)),
        '?' => (rgb(120, 120, 135), rgb(130, 130, 145)),
        _ => (rgb(240, 200, 80), rgb(170, 170, 180)),
    };
    // Layout: `M● name…   +84 −12`. Counts are dropped before the name gets
    // unreadably short.
    const PREFIX: usize = 3; // letter + agent marker + space
    const MIN_NAME: usize = 8;
    let show_counts = count_w > 0 && width >= PREFIX + MIN_NAME + 1 + count_w;
    let name_w = if show_counts {
        width - PREFIX - 1 - count_w
    } else {
        width.saturating_sub(PREFIX)
    };
    let name = truncate_smart(&changes_display_path(&file.path), name_w);
    let name_len = unicode_width::UnicodeWidthStr::width(name.as_str());

    let mut spans = vec![
        Span::styled(
            file.status.to_string(),
            Style::default().fg(letter_color).bold(),
        ),
        if agent {
            Span::styled("●", {
                let (r, g, b) = AGENT_DOT_COLOR;
                Style::default().fg(rgb(r, g, b))
            })
        } else {
            Span::raw(" ")
        },
        Span::raw(" "),
        Span::styled(name, Style::default().fg(path_color)),
    ];
    if show_counts {
        let (a, r) = line_counts(file).unwrap_or_default();
        let this_w = if a.is_empty() { 0 } else { a.chars().count() + 1 + r.chars().count() };
        let pad = width.saturating_sub(PREFIX + name_len + this_w);
        spans.push(Span::raw(" ".repeat(pad)));
        if !a.is_empty() {
            spans.push(Span::styled(a, Style::default().fg(rgb(90, 170, 100))));
            spans.push(Span::raw(" "));
            spans.push(Span::styled(r, Style::default().fg(rgb(200, 100, 95))));
        }
    }
    Line::from(spans)
}

/// Show the file name first: the tail of the path is what identifies it in a
/// narrow column. `crates/a/src/tool/mod.rs` becomes `tool/mod.rs` for
/// generic names and `turn_execution.rs` otherwise. Renames keep the target.
pub(super) fn changes_display_path(path: &str) -> String {
    let path = path.rsplit(" -> ").next().unwrap_or(path).trim_matches('"');
    let trimmed = path.trim_end_matches('/');
    let mut segments = trimmed.rsplit('/');
    let name = segments.next().unwrap_or(trimmed);
    let generic = matches!(
        name,
        "mod.rs" | "lib.rs" | "main.rs" | "index.ts" | "index.js" | "__init__.py" | "Cargo.toml"
    );
    let base = match (generic, segments.next()) {
        (true, Some(parent)) => format!("{parent}/{name}"),
        _ => name.to_string(),
    };
    if path.ends_with('/') {
        format!("{base}/")
    } else {
        base
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(files: &[(char, &str)], total: usize) -> GitInfo {
        GitInfo {
            branch: "main".to_string(),
            modified: files.len(),
            staged: 0,
            untracked: 0,
            ahead: 1,
            behind: 0,
            dirty_files: files.iter().map(|(s, p)| DirtyFile::new(*s, *p)).collect(),
            dirty_total: total,
            ..Default::default()
        }
    }

    fn text(lines: &[Line<'static>]) -> String {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn changes_lists_files_without_repeating_branch_or_counts() {
        let data = InfoWidgetData {
            git_info: Some(info(
                &[
                    ('M', "crates/x/src/agent/turn_execution.rs"),
                    ('?', "notes.md"),
                ],
                2,
            )),
            ..Default::default()
        };
        let out = text(&render_git_widget(&data, Rect::new(0, 0, 30, 5)));
        assert!(out.contains("M  turn_execution.rs"), "{out}");
        assert!(out.contains("?  notes.md"), "{out}");
        assert!(!out.contains("main"), "branch belongs to the status line: {out}");
        assert!(!out.contains("↑1"), "counts belong to the status line: {out}");
    }

    #[test]
    fn changes_overflow_counts_files_beyond_the_capture_cap() {
        let files: Vec<(char, &str)> = (0..10).map(|_| ('M', "a.rs")).collect();
        let git = info(&files, 23);
        let data = InfoWidgetData {
            git_info: Some(git.clone()),
            ..Default::default()
        };
        let lines = render_git_widget(&data, Rect::new(0, 0, 30, 10));
        assert_eq!(lines.len() as u16, changes_height(&git));
        assert!(text(&lines).contains("+19 more"), "{}", text(&lines));
    }

    #[test]
    fn clean_or_ahead_only_tree_has_no_changes_widget() {
        assert!(!changes_has_data(&info(&[], 0)));
        assert_eq!(changes_height(&info(&[], 0)), 0);
    }

    #[test]
    fn display_path_keeps_parent_for_generic_names_and_rename_targets() {
        assert_eq!(changes_display_path("crates/a/src/tool/mod.rs"), "tool/mod.rs");
        assert_eq!(changes_display_path("src/foo.rs"), "foo.rs");
        assert_eq!(changes_display_path("old.rs -> new/place.rs"), "place.rs");
        assert_eq!(changes_display_path("scratch/"), "scratch/");
    }

    #[test]
    fn porcelain_pairs_collapse_to_one_letter() {
        use crate::tui::app::helpers::porcelain_status_letter as l;
        assert_eq!(l(b'?', b'?'), '?');
        assert_eq!(l(b' ', b'M'), 'M');
        assert_eq!(l(b'M', b' '), 'M');
        assert_eq!(l(b'A', b' '), 'A');
        assert_eq!(l(b'A', b'M'), 'M');
        assert_eq!(l(b' ', b'D'), 'D');
        assert_eq!(l(b'R', b' '), 'R');
        assert_eq!(l(b'U', b'U'), 'U');
        assert_eq!(l(b'A', b'A'), 'U');
    }

    #[test]
    fn line_counts_align_right_and_agent_edits_get_a_dot() {
        let root = std::path::PathBuf::from("/repo");
        let git = GitInfo {
            dirty_files: vec![
                DirtyFile::new('M', "src/agent/turn_execution.rs").with_lines(84, 12),
                DirtyFile::new('M', "src/model_names.rs").with_lines(264, 8),
                DirtyFile::new('?', "notes.md").with_lines(5, 0),
            ],
            dirty_total: 3,
            repo_root: Some(root.clone()),
            ..Default::default()
        };
        let data = InfoWidgetData {
            git_info: Some(git),
            agent_edited: std::sync::Arc::new(
                [root.join("src/agent/turn_execution.rs")].into_iter().collect(),
            ),
            ..Default::default()
        };
        let lines = render_git_widget(&data, Rect::new(0, 0, 32, 5));
        let rows: Vec<String> = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert!(rows[0].starts_with("M● turn_execution.rs"), "{rows:#?}");
        assert!(rows[1].starts_with("M  model_names.rs"), "{rows:#?}");
        assert!(rows[0].ends_with("+84 −12"), "{rows:#?}");
        assert!(rows[1].ends_with("+264 −8"), "{rows:#?}");
        for r in &rows {
            assert_eq!(unicode_width::UnicodeWidthStr::width(r.as_str()), 32, "{r:?}");
        }
    }

    #[test]
    fn overflow_row_reports_totals_across_all_files() {
        let files: Vec<DirtyFile> =
            (0..10).map(|i| DirtyFile::new('M', format!("f{i}.rs")).with_lines(1, 1)).collect();
        let data = InfoWidgetData {
            git_info: Some(GitInfo {
                dirty_files: files,
                dirty_total: 23,
                added_total: 410,
                removed_total: 96,
                ..Default::default()
            }),
            ..Default::default()
        };
        let lines = render_git_widget(&data, Rect::new(0, 0, 32, 10));
        let last: String = lines.last().unwrap().spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(last.contains("+19 more") && last.ends_with("+410 −96 all"), "{last:?}");
    }

    #[test]
    fn narrow_width_drops_counts_before_the_name() {
        let data = InfoWidgetData {
            git_info: Some(GitInfo {
                dirty_files: vec![DirtyFile::new('M', "turn_execution.rs").with_lines(84, 12)],
                dirty_total: 1,
                ..Default::default()
            }),
            ..Default::default()
        };
        let row: String = render_git_widget(&data, Rect::new(0, 0, 16, 3))[0]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(!row.contains("+84"), "{row:?}");
        assert!(row.contains("turn_exec"), "{row:?}");
    }

    #[test]
    fn edited_paths_cover_edit_write_and_both_patch_formats() {
        use serde_json::json;
        assert_eq!(
            edited_paths_from_tool_call("edit", &json!({"file_path": "/r/a.rs"})),
            ["/r/a.rs"]
        );
        assert_eq!(
            edited_paths_from_tool_call("write", &json!({"file_path": "b.rs"})),
            ["b.rs"]
        );
        let codex = "*** Begin Patch\n*** Update File: x/c.rs\n@@\n-a\n+b\n*** Add File: d.rs\n+x\n*** End Patch";
        assert_eq!(
            edited_paths_from_tool_call("apply_patch", &json!({"patch_text": codex})),
            ["x/c.rs", "d.rs"]
        );
        let unified = "--- a/e.rs\n+++ b/e.rs\n@@\n-a\n+b\n";
        assert_eq!(
            edited_paths_from_tool_call("apply_patch", &json!({"patch_text": unified})),
            ["e.rs"]
        );
        assert!(edited_paths_from_tool_call("read", &json!({"file_path": "a.rs"})).is_empty());
        assert!(edited_paths_from_tool_call("bash", &json!({"command": "rm a"})).is_empty());
    }

    #[test]
    fn edited_paths_resolve_against_working_dir_and_match_repo_paths() {
        let p = resolve_edited_path("../src/./a.rs", Some("/home/me/jcode/crates"));
        assert_eq!(p, std::path::PathBuf::from("/home/me/jcode/src/a.rs"));
        let set: std::collections::HashSet<_> = [p].into_iter().collect();
        let root = std::path::Path::new("/home/me/jcode");
        assert!(is_agent_edited(Some(root), "src/a.rs", &set));
        assert!(!is_agent_edited(Some(root), "src/b.rs", &set));
        assert!(!is_agent_edited(None, "src/a.rs", &set));
    }

    #[test]
    fn numstat_parses_plain_binary_and_renamed_paths() {
        let m = crate::tui::app::helpers::parse_numstat(
            "84\t12\tsrc/a.rs\n-\t-\tlogo.png\n3\t1\tsrc/{old => new}/b.rs\n2\t2\told.rs => new.rs\n",
        );
        assert_eq!(m["src/a.rs"], (Some(84), Some(12)));
        assert_eq!(m["logo.png"], (None, None));
        assert_eq!(m["src/new/b.rs"], (Some(3), Some(1)));
        assert_eq!(m["new.rs"], (Some(2), Some(2)));
    }

    /// Real-repo probe: runs the production git gather against the working
    /// directory and renders the Changes widget. Run on demand with
    /// `cargo test -p jcode-tui -- --ignored real_repo_changes --nocapture`.
    #[test]
    #[ignore]
    fn real_repo_changes_widget() {
        let info = crate::tui::app::helpers::gather_git_info_inner().expect("inside a git repo");
        let data = InfoWidgetData {
            git_info: Some(info.clone()),
            ..Default::default()
        };
        println!("dirty_total={} +{} -{}", info.dirty_total, info.added_total, info.removed_total);
        for l in render_git_widget(&data, Rect::new(0, 0, 36, 5)) {
            println!("|{}|", l.spans.iter().map(|s| s.content.as_ref()).collect::<String>());
        }
        assert!(info.repo_root.is_some());
        let ordered: Vec<_> = info.dirty_files.iter().map(|f| f.modified_at).collect();
        assert!(ordered.windows(2).all(|w| w[0] >= w[1]), "newest first");
    }
}
