//! Activity timeline widget: a vertical, git-log-meets-todo visualization of
//! what the agent has been doing. Each tool the agent runs contributes an
//! "intent" node (sourced from the tool-call `intent` parameter); file edits
//! attach a `✎` marker, and new commits drop a `◆` node onto the same spine.
//!
//! The event store is process-global (mirroring the other info-widget caches)
//! and is fed off the render path from `App::note_tool_completed`.

use super::InfoWidgetData;
use super::text::clip;
use crate::tui::color_support::rgb;
use ratatui::prelude::*;
use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::Instant;

/// Maximum number of events retained in the rolling timeline.
const MAX_EVENTS: usize = 48;

/// What a single timeline entry represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimelineEventKind {
    /// A generic agent action, labeled by its tool-call intent.
    Intent,
    /// A file edit/write, labeled by the touched file.
    Edit,
    /// A new git commit (HEAD advanced).
    Commit,
}

/// A single point on the activity timeline.
#[derive(Debug, Clone)]
pub struct TimelineEvent {
    pub kind: TimelineEventKind,
    /// Primary text: the intent (Intent/Edit) or the commit subject (Commit).
    pub label: String,
    /// Secondary text: the edited file (Edit) or the short hash (Commit).
    pub detail: Option<String>,
    /// When the event was recorded (used only for relative ordering today).
    pub at: Instant,
}

struct TimelineStore {
    events: VecDeque<TimelineEvent>,
    /// Last git HEAD short hash we observed, so we can detect new commits.
    last_head: Option<String>,
    /// Whether `last_head` has been seeded at least once. Until then we never
    /// emit a Commit event, so the pre-existing HEAD is not mistaken for a new
    /// commit the first time the agent touches the repo.
    head_seeded: bool,
}

impl TimelineStore {
    fn new() -> Self {
        Self {
            events: VecDeque::with_capacity(MAX_EVENTS),
            last_head: None,
            head_seeded: false,
        }
    }

    fn push(&mut self, event: TimelineEvent) {
        // Collapse consecutive duplicates so a burst of identical intents (or
        // repeated edits to the same file under one intent) reads as one node.
        if let Some(last) = self.events.back() {
            if last.kind == event.kind && last.label == event.label && last.detail == event.detail {
                return;
            }
        }
        self.events.push_back(event);
        while self.events.len() > MAX_EVENTS {
            self.events.pop_front();
        }
    }
}

static TIMELINE: Mutex<Option<TimelineStore>> = Mutex::new(None);

fn with_store<R>(f: impl FnOnce(&mut TimelineStore) -> R) -> Option<R> {
    let mut guard = TIMELINE.lock().ok()?;
    if guard.is_none() {
        *guard = Some(TimelineStore::new());
    }
    guard.as_mut().map(f)
}

/// Turn a snake/kebab tool name into a friendlier fallback label when a tool
/// call has no explicit intent (e.g. `apply_patch` -> "apply patch").
fn humanize_tool(name: &str) -> String {
    name.replace(['_', '-'], " ")
}

/// Record a single tool invocation on the timeline.
///
/// `intent` is the tool-call `intent` parameter (the agent's stated reason for
/// the call); `edit_file` is `Some(path)` for file-mutating tools, which makes
/// the entry render as an edit marker instead of a plain intent node.
pub fn timeline_record_tool(name: &str, intent: Option<&str>, edit_file: Option<&str>) {
    let label = intent
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| humanize_tool(name));

    let (kind, detail) = match edit_file {
        Some(path) => (TimelineEventKind::Edit, Some(short_path(path))),
        None => (TimelineEventKind::Intent, None),
    };

    let _ = with_store(|store| {
        store.push(TimelineEvent {
            kind,
            label,
            detail,
            at: Instant::now(),
        });
    });
}

/// Take the trailing path component so a deep edit path stays readable in a
/// narrow margin widget (`crates/foo/src/bar.rs` -> `bar.rs`).
fn short_path(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    trimmed
        .rsplit(['/', '\\'])
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or(trimmed)
        .to_string()
}

/// Check whether HEAD advanced and, if so, drop a commit node on the timeline.
///
/// Runs git off-thread so it never touches the render path. The first time it
/// observes a HEAD it only seeds the baseline (no event), so the repo's
/// pre-existing commit is not surfaced as if the agent had just made it.
pub fn timeline_record_commit_if_changed() {
    std::thread::spawn(|| {
        use std::process::Command;
        let output = Command::new("git")
            .args(["log", "-1", "--no-color", "--pretty=format:%h\x1f%s"])
            .output()
            .ok();
        let Some(output) = output else {
            return;
        };
        if !output.status.success() {
            return;
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let mut parts = text.splitn(2, '\x1f');
        let hash = parts.next().unwrap_or("").trim().to_string();
        let subject = parts.next().unwrap_or("").trim().to_string();
        if hash.is_empty() {
            return;
        }

        let _ = with_store(|store| {
            let changed = store.last_head.as_deref() != Some(hash.as_str());
            let should_emit = store.head_seeded && changed;
            store.last_head = Some(hash.clone());
            store.head_seeded = true;
            if should_emit {
                store.push(TimelineEvent {
                    kind: TimelineEventKind::Commit,
                    label: if subject.is_empty() {
                        "commit".to_string()
                    } else {
                        subject
                    },
                    detail: Some(hash),
                    at: Instant::now(),
                });
            }
        });
    });
}

/// Snapshot the current timeline for the info widget (oldest first).
pub fn timeline_snapshot() -> Vec<TimelineEvent> {
    with_store(|store| store.events.iter().cloned().collect()).unwrap_or_default()
}

/// Test-only: clear the timeline so tests start from a known state.
#[cfg(test)]
pub fn clear_timeline_for_tests() {
    let _ = with_store(|store| {
        store.events.clear();
        store.last_head = None;
        store.head_seeded = false;
    });
}

const NOW_MARKER: &str = " ◀";

fn render_event_line(ev: &TimelineEvent, width: usize, is_last: bool) -> Line<'static> {
    let reserved = if is_last { NOW_MARKER.chars().count() } else { 0 };
    let mut spans: Vec<Span<'static>> = Vec::new();

    match ev.kind {
        TimelineEventKind::Intent => {
            spans.push(Span::styled("● ", Style::default().fg(rgb(130, 170, 250))));
            let max = width.saturating_sub(2 + reserved).max(4);
            spans.push(Span::styled(
                clip(&ev.label, max),
                Style::default().fg(rgb(195, 195, 210)),
            ));
        }
        TimelineEventKind::Edit => {
            spans.push(Span::styled("│ ", Style::default().fg(rgb(80, 80, 95))));
            spans.push(Span::styled("✎ ", Style::default().fg(rgb(240, 200, 80))));
            let text = ev.detail.as_deref().unwrap_or(&ev.label);
            let max = width.saturating_sub(4 + reserved).max(4);
            spans.push(Span::styled(
                clip(text, max),
                Style::default().fg(rgb(150, 150, 165)),
            ));
        }
        TimelineEventKind::Commit => {
            spans.push(Span::styled("◆ ", Style::default().fg(rgb(120, 200, 140))));
            let mut used = 2usize;
            if let Some(hash) = ev.detail.as_deref() {
                let hash_disp = format!("{} ", hash);
                used += hash_disp.chars().count();
                spans.push(Span::styled(
                    hash_disp,
                    Style::default().fg(rgb(110, 200, 140)),
                ));
            }
            let max = width.saturating_sub(used + reserved).max(4);
            spans.push(Span::styled(
                clip(&ev.label, max),
                Style::default().fg(rgb(185, 185, 195)),
            ));
        }
    }

    if is_last {
        spans.push(Span::styled(NOW_MARKER, Style::default().fg(rgb(255, 200, 100))));
    }

    Line::from(spans)
}

/// Render the full activity timeline (header + spine), newest at the bottom.
pub(super) fn render_timeline_widget(data: &InfoWidgetData, inner: Rect) -> Vec<Line<'static>> {
    let events = &data.timeline;
    if events.is_empty() {
        return Vec::new();
    }

    let width = inner.width as usize;
    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(Line::from(vec![Span::styled(
        "Activity",
        Style::default().fg(rgb(180, 180, 190)).bold(),
    )]));

    let budget = inner.height.saturating_sub(1).max(1) as usize;
    let start = events.len().saturating_sub(budget);
    let visible = &events[start..];
    let last_idx = visible.len().saturating_sub(1);
    let hidden = start;

    for (i, ev) in visible.iter().enumerate() {
        lines.push(render_event_line(ev, width, i == last_idx));
    }

    if hidden > 0 && (lines.len() as u16) < inner.height {
        // Only if we somehow have spare room (rare) note the truncation.
        lines.insert(
            1,
            Line::from(vec![Span::styled(
                format!("  +{} earlier", hidden),
                Style::default().fg(rgb(100, 100, 115)),
            )]),
        );
    }

    lines
}

/// Compact one-line summary used when the timeline is space-constrained.
pub(super) fn render_timeline_compact(data: &InfoWidgetData) -> Vec<Line<'static>> {
    let events = &data.timeline;
    if events.is_empty() {
        return Vec::new();
    }
    let edits = events
        .iter()
        .filter(|e| e.kind == TimelineEventKind::Edit)
        .count();
    let commits = events
        .iter()
        .filter(|e| e.kind == TimelineEventKind::Commit)
        .count();
    let last = events
        .iter()
        .rev()
        .find(|e| e.kind != TimelineEventKind::Commit)
        .map(|e| e.label.as_str())
        .unwrap_or("working");

    let mut spans = vec![
        Span::styled("⟳ ", Style::default().fg(rgb(130, 170, 250))),
        Span::styled(
            clip(last, 24),
            Style::default().fg(rgb(185, 185, 200)),
        ),
    ];
    if edits > 0 {
        spans.push(Span::styled(
            format!(" · {} edits", edits),
            Style::default().fg(rgb(150, 150, 165)),
        ));
    }
    if commits > 0 {
        spans.push(Span::styled(
            format!(" · {} commits", commits),
            Style::default().fg(rgb(120, 200, 140)),
        ));
    }
    vec![Line::from(spans)]
}

/// Number of content rows (excluding borders) the timeline wants given a height
/// budget. Header + up to `max_rows-1` events.
pub(super) fn timeline_content_height(data: &InfoWidgetData, max_rows: u16) -> u16 {
    if data.timeline.is_empty() {
        return 0;
    }
    let events = data.timeline.len() as u16;
    (1 + events).min(max_rows.max(1))
}
