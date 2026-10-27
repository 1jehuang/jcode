//! Changes widget: the detail layer behind the status line's git segment.
//!
//! The overscroll status line already owns the branch and the dirty/ahead
//! counts (`main ~3 ?2 ↑1`). This widget never repeats those. It answers the
//! follow-up question instead: *which* files are dirty.

use super::frame::{self, Framed};
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
    let rel = repo_path
        .rsplit(" -> ")
        .next()
        .unwrap_or(repo_path)
        .trim_matches('"');
    edited.contains(&root.join(rel.trim_end_matches('/')))
}

/// Color of the agent-edit marker, shared by the rows and the legend.
pub(super) const AGENT_DOT_COLOR: (u8, u8, u8) = (186, 139, 255);

/// Border legend explaining the agent marker, shown only when at least one
/// visible row carries it so it never labels something absent.
pub(super) fn changes_legend(data: &InfoWidgetData, inner_height: u16) -> Option<Line<'static>> {
    if data.dual_git_info.is_some() {
        return None;
    }
    let info = data.git_info.as_ref()?;
    let rows = (inner_height as usize).min(CHANGES_MAX_FILES);
    let root = info.repo_root.as_deref();
    let any = info
        .dirty_files
        .iter()
        .take(rows)
        .any(|f| is_agent_edited(root, &f.path, &data.agent_edited));
    any.then(|| {
        let (r, g, b) = AGENT_DOT_COLOR;
        Line::from(vec![
            Span::styled("●", Style::default().fg(rgb(r, g, b))),
            Span::styled(" agent", Style::default().fg(rgb(130, 130, 145))),
        ])
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
    // Overview section: up to CHANGES_MAX_FILES rows, the last of which
    // becomes `+N more` when files overflow.
    let total = info.dirty_total.max(info.dirty_files.len());
    let shown = info.dirty_files.len().min(CHANGES_MAX_FILES);
    if total > shown {
        CHANGES_MAX_FILES.min(shown + 1) as u16
    } else {
        shown as u16
    }
}

/// Standalone Changes widget. Border layout: `Changes` top-left, all-file
/// `+410 −96` totals top-right, `+N more` bottom-left, agent legend
/// bottom-right. Every body row is a file. The dirty-file count stays on the
/// status line.
pub(super) fn render_changes_framed(data: &InfoWidgetData, inner: Rect) -> Framed {
    if let Some(dual) = data.dual_git_info.as_ref() {
        let source = if dual.local_is_source {
            "local → cloud"
        } else {
            "local not source"
        };
        return Framed::body(render_git_widget(data, inner))
            .title(frame::label("Repositories"))
            .footer(frame::dim(source));
    }
    let Some(info) = &data.git_info else {
        return Framed::default();
    };
    if !changes_has_data(info) {
        return Framed::default();
    }
    let total = info.dirty_total.max(info.dirty_files.len());
    let max_files = (inner.height as usize).min(CHANGES_MAX_FILES);
    let lines = changes_file_lines(data, info, max_files, inner.width as usize);
    let hidden = total.saturating_sub(lines.len());

    let mut framed = Framed::body(lines).title(frame::label("Changes"));
    if info.added_total + info.removed_total > 0 {
        framed = framed.title_right(Line::from(vec![
            Span::styled(
                format!("+{}", info.added_total),
                Style::default().fg(rgb(90, 170, 100)),
            ),
            Span::styled(
                format!(" −{}", info.removed_total),
                Style::default().fg(rgb(200, 100, 95)),
            ),
        ]));
    }
    if hidden > 0 {
        framed = framed.footer(frame::more(hidden));
    } else {
        let directories: std::collections::HashSet<_> = info
            .dirty_files
            .iter()
            .flat_map(|f| f.path.split(" -> "))
            .map(|path| {
                std::path::Path::new(path.trim_matches('"'))
                    .parent()
                    .unwrap_or(std::path::Path::new(""))
            })
            .collect();
        let count = directories.len();
        framed = framed.footer(frame::dim(format!(
            "{count} {}",
            if count == 1 { "dir" } else { "dirs" }
        )));
    }
    if let Some(legend) = changes_legend(data, inner.height) {
        if hidden == 0 {
            framed.footer = None;
        }
        framed = framed.footer_right(legend);
    }
    framed
}

fn changes_file_lines(
    data: &InfoWidgetData,
    info: &GitInfo,
    max_files: usize,
    w: usize,
) -> Vec<Line<'static>> {
    let shown: Vec<&DirtyFile> = info.dirty_files.iter().take(max_files).collect();
    // One shared column width for `+N −M`, so the counts line up.
    let count_w = shown
        .iter()
        .map(|f| line_counts(f).map_or(0, |(a, r)| a.chars().count() + 1 + r.chars().count()))
        .max()
        .unwrap_or(0);
    let root = info.repo_root.as_deref();
    shown
        .iter()
        .map(|file| {
            let agent = is_agent_edited(root, &file.path, &data.agent_edited);
            changes_file_line(file, agent, count_w, w)
        })
        .collect()
}

/// Overview section rows (no border of its own): files plus an inline
/// `+N more` row carrying the all-file totals.
pub(super) fn render_git_widget(data: &InfoWidgetData, inner: Rect) -> Vec<Line<'static>> {
    if let Some(dual) = &data.dual_git_info {
        return render_dual_git(data, dual, inner);
    }
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

    let mut lines = changes_file_lines(data, info, max_files, w);

    let hidden = total.saturating_sub(lines.len());
    if hidden > 0 {
        let dim = Style::default().fg(rgb(100, 100, 115));
        let mut spans = vec![Span::styled(format!("  +{hidden} more"), dim)];
        if info.added_total + info.removed_total > 0 {
            let totals = format!("+{} −{} all", info.added_total, info.removed_total);
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

fn truncate_cells(text: &str, width: usize) -> String {
    use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
    if text.width() <= width {
        return text.to_string();
    }
    if width == 0 {
        return String::new();
    }
    let mut used = 0;
    let mut out = String::new();
    for ch in text.chars() {
        let cells = ch.width().unwrap_or(0);
        if used + cells > width - 1 {
            break;
        }
        out.push(ch);
        used += cells;
    }
    out.push('…');
    out
}

fn repo_rows(state: &super::super::cloud_git::RepoState) -> u16 {
    match state {
        super::super::cloud_git::RepoState::Ready(info) => 1 + changes_height(info),
        _ => 1,
    }
}

pub(super) fn dual_git_height(dual: &super::super::cloud_git::DualGitInfo) -> u16 {
    repo_rows(&dual.cloud) + repo_rows(&dual.local)
}

fn render_dual_git(
    data: &InfoWidgetData,
    dual: &super::super::cloud_git::DualGitInfo,
    inner: Rect,
) -> Vec<Line<'static>> {
    use super::super::cloud_git::RepoState;
    // Reserve a heading for each machine before allocating file rows. A busy
    // VM must never push the local state out of the widget (or vice versa).
    let budget = inner.height.saturating_sub(2) as usize;
    let cloud_wanted = repo_rows(&dual.cloud).saturating_sub(1) as usize;
    let local_wanted = repo_rows(&dual.local).saturating_sub(1) as usize;
    let cloud_budget = cloud_wanted.min(budget.saturating_sub(local_wanted.min(budget / 2)));
    let local_budget = local_wanted.min(budget.saturating_sub(cloud_budget));
    let width = inner.width as usize;
    let safe = |text: &str| -> String {
        let text: String = text
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect();
        truncate_cells(&text, width)
    };
    let mut lines = Vec::new();
    for (label, state, row_budget, cloud) in [
        ("Cloud VM", &dual.cloud, cloud_budget, true),
        (
            if dual.local_is_source {
                "Local"
            } else {
                "Local (launch dir)"
            },
            &dual.local,
            local_budget,
            false,
        ),
    ] {
        let summary = match state {
            RepoState::Loading => "loading…".to_string(),
            RepoState::Unavailable => "unavailable".to_string(),
            RepoState::NoRepository => "no repository".to_string(),
            RepoState::Ready(info) => {
                let counts = if info.dirty_total == 0 {
                    "clean".into()
                } else {
                    format!("~{} +{} ?{}", info.modified, info.staged, info.untracked)
                };
                if info.ahead > 0 || info.behind > 0 {
                    format!("{} {counts} ↑{} ↓{}", info.branch, info.ahead, info.behind)
                } else {
                    format!("{} {counts}", info.branch)
                }
            }
        };
        lines.push(Line::from(Span::styled(
            safe(&format!("{label}: {summary}")),
            Style::default().fg(rgb(140, 180, 255)),
        )));
        if let RepoState::Ready(info) = state {
            let overflow = info.dirty_total > row_budget;
            let shown = if overflow {
                row_budget.saturating_sub(1)
            } else {
                row_budget
            };
            for file in info.dirty_files.iter().take(shown) {
                let agent = cloud
                    && is_agent_edited(info.repo_root.as_deref(), &file.path, &data.agent_edited);
                let mut file = file.clone();
                file.path = file
                    .path
                    .chars()
                    .map(|c| if c.is_control() { ' ' } else { c })
                    .collect();
                // Existing file rows have a three-cell prefix. Bound even
                // widths smaller than that prefix without wrapping chrome.
                file.path =
                    truncate_cells(&changes_display_path(&file.path), width.saturating_sub(3));
                if width < 3 {
                    lines.push(Line::from(safe(&file.status.to_string())));
                } else {
                    lines.push(changes_file_line(&file, agent, 0, width));
                }
            }
            if overflow && row_budget > 0 {
                lines.push(Line::from(safe(&format!(
                    "  +{} more",
                    info.dirty_total.saturating_sub(shown)
                ))));
            }
        }
    }
    lines.truncate(inner.height as usize);
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
        let this_w = if a.is_empty() {
            0
        } else {
            a.chars().count() + 1 + r.chars().count()
        };
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

    #[test]
    fn changes_footer_aggregates_directories_without_repeating_file_counts() {
        let data = InfoWidgetData {
            git_info: Some(info(
                &[('M', "src/a.rs"), ('M', "src/b.rs"), ('?', "tests/c.rs")],
                3,
            )),
            ..Default::default()
        };
        let card = render_changes_framed(&data, Rect::new(0, 0, 40, 5));
        assert_eq!(card.footer.unwrap().to_string(), "2 dirs");
        let mut dual_data = data;
        dual_data.dual_git_info = Some(super::super::super::cloud_git::DualGitInfo {
            local_is_source: true,
            local: super::super::super::cloud_git::RepoState::Loading,
            cloud: super::super::super::cloud_git::RepoState::Loading,
        });
        assert_eq!(
            render_changes_framed(&dual_data, Rect::new(0, 0, 40, 5))
                .footer
                .unwrap()
                .to_string(),
            "local → cloud"
        );
    }

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
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
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
        assert!(
            !out.contains("main"),
            "branch belongs to the status line: {out}"
        );
        assert!(
            !out.contains("↑1"),
            "counts belong to the status line: {out}"
        );
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
        assert_eq!(
            changes_display_path("crates/a/src/tool/mod.rs"),
            "tool/mod.rs"
        );
        assert_eq!(changes_display_path("src/foo.rs"), "foo.rs");
        assert_eq!(changes_display_path("old.rs -> new/place.rs"), "place.rs");
        assert_eq!(changes_display_path("scratch/"), "scratch/");
    }

    #[test]
    fn dual_git_states_are_labeled_width_safe_and_mark_only_cloud_edits() {
        use crate::tui::cloud_git::{DualGitInfo, RepoState};
        let repo = GitInfo {
            branch: "main界".into(),
            dirty_total: 1,
            dirty_files: vec![DirtyFile::new('M', "shared\n界name.rs")],
            repo_root: Some("/same".into()),
            modified: 1,
            ..Default::default()
        };
        let data = InfoWidgetData {
            dual_git_info: Some(DualGitInfo {
                local_is_source: true,
                local: RepoState::Ready(repo.clone()),
                cloud: RepoState::Ready(repo),
            }),
            agent_edited: std::sync::Arc::new(
                [std::path::PathBuf::from("/same/shared\n界name.rs")]
                    .into_iter()
                    .collect(),
            ),
            ..Default::default()
        };
        assert!(data.has_data_for(super::super::WidgetKind::GitStatus));
        let rows: Vec<String> = render_git_widget(&data, Rect::new(0, 0, 50, 10))
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert!(rows[0].starts_with("Cloud VM: main界"));
        assert!(rows[1].contains('●'));
        assert!(rows[2].starts_with("Local: main界"));
        assert!(!rows[3].contains('●'));
        for width in 0..=50 {
            let lines = render_git_widget(&data, Rect::new(0, 0, width, 10));
            for line in lines {
                let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
                assert!(
                    unicode_width::UnicodeWidthStr::width(text.as_str()) <= width as usize,
                    "{width}: {text:?}"
                );
                assert!(!text.chars().any(char::is_control));
            }
        }
    }

    #[test]
    fn dual_git_keeps_clean_missing_and_unavailable_repositories_visible() {
        use crate::tui::cloud_git::{DualGitInfo, RepoState};
        for cloud in [
            RepoState::Loading,
            RepoState::Unavailable,
            RepoState::NoRepository,
            RepoState::Ready(GitInfo {
                branch: "clean-branch".into(),
                ..Default::default()
            }),
        ] {
            let data = InfoWidgetData {
                dual_git_info: Some(DualGitInfo {
                    local_is_source: true,
                    cloud,
                    local: RepoState::Unavailable,
                }),
                ..Default::default()
            };
            assert_eq!(super::super::changes_section_height(&data), 2);
            let rows = render_git_widget(&data, Rect::new(0, 0, 70, 2));
            assert_eq!(rows.len(), 2);
            let text: String = rows
                .iter()
                .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
                .collect();
            assert!(text.contains("Cloud VM:"));
            assert!(text.contains("Local: unavailable"));
        }
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
                [root.join("src/agent/turn_execution.rs")]
                    .into_iter()
                    .collect(),
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
            assert_eq!(
                unicode_width::UnicodeWidthStr::width(r.as_str()),
                32,
                "{r:?}"
            );
        }
    }

    #[test]
    fn overflow_row_reports_totals_across_all_files() {
        let files: Vec<DirtyFile> = (0..10)
            .map(|i| DirtyFile::new('M', format!("f{i}.rs")).with_lines(1, 1))
            .collect();
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
        let last: String = lines
            .last()
            .unwrap()
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(
            last.contains("+19 more") && last.ends_with("+410 −96 all"),
            "{last:?}"
        );
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
        println!(
            "dirty_total={} +{} -{}",
            info.dirty_total, info.added_total, info.removed_total
        );
        for l in render_git_widget(&data, Rect::new(0, 0, 36, 5)) {
            println!(
                "|{}|",
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            );
        }
        assert!(info.repo_root.is_some());
        let ordered: Vec<_> = info.dirty_files.iter().map(|f| f.modified_at).collect();
        assert!(ordered.windows(2).all(|w| w[0] >= w[1]), "newest first");
    }
}
