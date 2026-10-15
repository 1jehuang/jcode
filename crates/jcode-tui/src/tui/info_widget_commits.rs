//! Commits widget: what has landed on the current branch recently.
//!
//! The Changes widget answers "what is dirty". This one answers "what did we
//! just commit, and has it been pushed". Border layout: `Commits` top-left,
//! the unpushed count top-right, overflow bottom-left.
//!
//! ```text
//! ╭ Commits ──────────────────── ↑2 unpushed ╮
//! │● e4b4d3a swarm dock lists each ag… 2m  │
//! │● e205d0f info widgets put headers… 14m │
//! │  4f6bf8e replay reasoning before t… 1h │
//! ╰ +5 more ─────────────────────────────────╯
//! ```

use super::frame::{self, Framed};
use super::text::truncate_smart;
use super::{GitInfo, InfoWidgetData, RecentCommit};
use crate::tui::color_support::rgb;
use ratatui::prelude::*;
use unicode_width::UnicodeWidthStr;

/// Most commits listed before the rest collapse into the footer.
pub(super) const COMMITS_MAX_ROWS: usize = 5;

/// Hide the widget once the newest commit is older than this: stale history
/// is not worth margin space.
const COMMITS_FRESH_SECS: i64 = 24 * 3600;

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub(crate) fn commits_has_data(info: &GitInfo) -> bool {
    commits_has_data_at(info, now_secs())
}

/// Rows the overview Commits section lists before collapsing the rest into an
/// inline `+N more` row.
pub(crate) const OVERVIEW_COMMITS_MAX_ROWS: usize = 3;

/// Overview Commits section: a header line (`Commits ↑N unpushed`) plus up to
/// [`OVERVIEW_COMMITS_MAX_ROWS`] recent commits and an overflow row. Mirrors
/// [`super::info_widget_overview::compact_commits_height`].
pub(super) fn render_commits_compact(info: &GitInfo, width: usize) -> Vec<Line<'static>> {
    let now = now_secs();
    if !commits_has_data_at(info, now) {
        return Vec::new();
    }

    let mut lines = Vec::new();

    let unpushed = info
        .recent_commits
        .iter()
        .filter(|c| c.unpushed)
        .count()
        .max(info.ahead);
    let mut header = vec![Span::styled(
        "Commits",
        Style::default().fg(rgb(180, 180, 190)).bold(),
    )];
    if unpushed > 0 {
        header.push(Span::raw("  "));
        header.push(Span::styled(
            format!("↑{unpushed} unpushed"),
            Style::default().fg(rgb(140, 180, 255)),
        ));
    }
    lines.push(Line::from(header));

    let rows = OVERVIEW_COMMITS_MAX_ROWS.min(info.recent_commits.len());
    for commit in &info.recent_commits[..rows] {
        lines.push(commit_line(commit, width, now));
    }

    let hidden = info.recent_commits.len().saturating_sub(rows);
    if hidden > 0 {
        lines.push(Line::from(vec![Span::styled(
            format!("  +{hidden} more"),
            Style::default().fg(rgb(110, 110, 125)),
        )]));
    }

    lines
}

fn commits_has_data_at(info: &GitInfo, now: i64) -> bool {
    info.recent_commits.first().is_some_and(|newest| {
        newest.unpushed || now.saturating_sub(newest.timestamp) < COMMITS_FRESH_SECS
    })
}

fn short_age(secs: i64) -> String {
    let secs = secs.max(0);
    if secs < 60 {
        "now".to_string()
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86_400)
    }
}

pub(super) fn render_commits_widget(data: &InfoWidgetData, inner: Rect) -> Framed {
    render_commits_at(data, inner, now_secs())
}

fn render_commits_at(data: &InfoWidgetData, inner: Rect, now: i64) -> Framed {
    let Some(info) = &data.git_info else {
        return Framed::default();
    };
    if !commits_has_data_at(info, now) {
        return Framed::default();
    }
    let rows = (inner.height as usize)
        .clamp(1, COMMITS_MAX_ROWS)
        .min(info.recent_commits.len());
    let width = inner.width as usize;
    let lines: Vec<Line<'static>> = info.recent_commits[..rows]
        .iter()
        .map(|c| commit_line(c, width, now))
        .collect();

    let mut framed = Framed::body(lines).title(frame::label("Commits"));
    let unpushed = info.recent_commits.iter().filter(|c| c.unpushed).count();
    // `ahead` counts past the fetched window; prefer it when larger.
    let unpushed = unpushed.max(info.ahead);
    if unpushed > 0 {
        framed = framed.title_right(Span::styled(
            format!("↑{unpushed} unpushed"),
            Style::default().fg(rgb(140, 180, 255)),
        ));
    }
    let hidden = info.recent_commits.len() - rows;
    if hidden > 0 {
        framed = framed.footer(frame::more(hidden));
    }
    framed
}

fn commit_line(commit: &RecentCommit, width: usize, now: i64) -> Line<'static> {
    let marker = if commit.unpushed {
        Span::styled("● ", Style::default().fg(rgb(140, 180, 255)))
    } else {
        Span::styled("  ", Style::default())
    };
    let hash = format!("{} ", commit.hash);
    let age = short_age(now - commit.timestamp);
    let stat = match (commit.added, commit.removed) {
        (Some(a), Some(r)) => Some((format!("+{a}"), format!(" −{r}"))),
        _ => None,
    };
    let stat_w = stat
        .as_ref()
        .map(|(a, r)| UnicodeWidthStr::width(a.as_str()) + UnicodeWidthStr::width(r.as_str()) + 1)
        .unwrap_or(0);
    let age_w = UnicodeWidthStr::width(age.as_str()) + 1;
    let fixed = 2 + UnicodeWidthStr::width(hash.as_str());

    // The subject is the point of the row: line stats only appear when it
    // keeps a comfortable width, and the age goes next.
    let min_subject = 12;
    let roomy_subject = 26;
    let (show_stat, show_age) = if width >= fixed + roomy_subject + stat_w + age_w {
        (stat.is_some(), true)
    } else if width >= fixed + min_subject + age_w {
        (false, true)
    } else {
        (false, false)
    };
    let right_w = if show_stat { stat_w } else { 0 } + if show_age { age_w } else { 0 };
    let subject_w = width.saturating_sub(fixed + right_w);
    let subject = truncate_smart(&commit.subject, subject_w.max(1));
    let pad = subject_w.saturating_sub(UnicodeWidthStr::width(subject.as_str()));

    let mut spans = vec![
        marker,
        Span::styled(hash, Style::default().fg(rgb(200, 170, 90))),
        Span::styled(
            subject,
            Style::default().fg(if commit.unpushed {
                rgb(210, 210, 220)
            } else {
                rgb(160, 160, 170)
            }),
        ),
        Span::raw(" ".repeat(pad)),
    ];
    if show_stat && let Some((a, r)) = stat {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(a, Style::default().fg(rgb(90, 170, 100))));
        spans.push(Span::styled(r, Style::default().fg(rgb(200, 100, 95))));
    }
    if show_age {
        spans.push(Span::styled(
            format!(" {age}"),
            Style::default().fg(rgb(120, 120, 130)),
        ));
    }
    let line = Line::from(spans);
    if line.width() <= width {
        line
    } else {
        frame::fit(&line, width).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000;

    fn commit(hash: &str, subject: &str, ago: i64, unpushed: bool) -> RecentCommit {
        RecentCommit {
            hash: hash.to_string(),
            subject: subject.to_string(),
            timestamp: NOW - ago,
            unpushed,
            added: Some(120),
            removed: Some(46),
        }
    }

    fn data(commits: Vec<RecentCommit>, ahead: usize) -> InfoWidgetData {
        InfoWidgetData {
            git_info: Some(GitInfo {
                ahead,
                recent_commits: commits,
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn text(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn lists_commits_with_unpushed_marker_and_border_count() {
        let d = data(
            vec![
                commit("e4b4d3a", "swarm dock lists each agent", 120, true),
                commit("e205d0f", "info widgets put headers on borders", 840, true),
                commit("4f6bf8e", "replay reasoning", 4000, false),
            ],
            2,
        );
        let f = render_commits_at(&d, Rect::new(0, 0, 60, 8), NOW);
        assert_eq!(f.lines.len(), 3);
        let first = text(&f.lines[0]);
        assert!(first.starts_with("● e4b4d3a swarm dock"), "{first}");
        assert!(first.ends_with(" 2m"), "{first}");
        assert!(first.contains("+120 −46"), "{first}");
        assert!(text(&f.lines[2]).starts_with("  4f6bf8e"));
        assert!(text(f.title_right.as_ref().unwrap()).contains("↑2 unpushed"));
        for line in &f.lines {
            assert!(line.width() <= 60);
        }
    }

    #[test]
    fn overflow_goes_to_footer() {
        let commits = (0..8)
            .map(|i| commit(&format!("abc{i}"), "work", 60 * i, false))
            .collect();
        let f = render_commits_at(&data(commits, 0), Rect::new(0, 0, 40, 20), NOW);
        assert_eq!(f.lines.len(), COMMITS_MAX_ROWS);
        assert!(text(f.footer.as_ref().unwrap()).contains("+3 more"));
    }

    #[test]
    fn stale_pushed_history_hides_the_widget() {
        let d = data(vec![commit("aaa", "old", 3 * 86_400, false)], 0);
        assert!(render_commits_at(&d, Rect::new(0, 0, 40, 5), NOW).is_empty());
        // Unpushed work always shows, however old.
        let d = data(vec![commit("aaa", "old", 3 * 86_400, true)], 1);
        assert!(!render_commits_at(&d, Rect::new(0, 0, 40, 5), NOW).is_empty());
    }

    #[test]
    fn narrow_widths_drop_stats_then_age_and_never_overflow() {
        let d = data(
            vec![commit("e4b4d3a", "a long subject line here", 120, true)],
            1,
        );
        for w in [0u16, 1, 8, 16, 24, 30, 60] {
            let f = render_commits_at(&d, Rect::new(0, 0, w, 3), NOW);
            for line in &f.lines {
                assert!(line.width() <= usize::from(w), "w={w}: {}", text(line));
            }
        }
        let f = render_commits_at(&d, Rect::new(0, 0, 30, 3), NOW);
        let row = text(&f.lines[0]);
        assert!(!row.contains('+'), "stats dropped first: {row}");
        assert!(row.ends_with(" 2m"), "age kept: {row}");
    }
}

#[cfg(test)]
mod parse_tests {
    use crate::tui::app::helpers::parse_recent_commits;

    #[test]
    fn parses_git_log_shortstat_records() {
        let text = "\x1ee4b4d3ad6\x1f1790663328\x1ftui: swarm dock\n\n 3 files changed, 363 insertions(+), 35 deletions(-)\n\
                    \x1efdad4643d\x1f1790662972\x1fadd gallery\n\n 1 file changed, 209 insertions(+)\n\
                    \x1eaaaaaaa\x1f1790660000\x1fempty commit\n";
        let commits = parse_recent_commits(text, 2);
        assert_eq!(commits.len(), 3);
        assert_eq!(commits[0].hash, "e4b4d3ad6");
        assert_eq!(commits[0].subject, "tui: swarm dock");
        assert_eq!(commits[0].timestamp, 1_790_663_328);
        assert_eq!(
            (commits[0].added, commits[0].removed),
            (Some(363), Some(35))
        );
        assert_eq!((commits[1].added, commits[1].removed), (Some(209), Some(0)));
        assert_eq!((commits[2].added, commits[2].removed), (None, None));
        assert!(commits[0].unpushed && commits[1].unpushed && !commits[2].unpushed);
    }
}
