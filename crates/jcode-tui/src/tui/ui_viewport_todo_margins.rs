//! Pinned todo bands and viewport margins.

use super::*;
use crate::config::TodoHeaderMode;

/// Use the exact pinned-card identity and context for both drawing and queries.
fn project_pinned_todo_card<R>(
    payload: Option<&str>,
    diff_mode: crate::config::DiffDisplayMode,
    width: u16,
    mode: TodoHeaderMode,
    project: impl FnOnce(&[Line<'static>]) -> R,
) -> R {
    if width < 16 || mode == TodoHeaderMode::Hidden {
        return project(&[]);
    }
    let Some(payload) = payload else {
        return project(&[]);
    };
    if payload.trim().is_empty() {
        return project(&[]);
    }
    // Distinct title avoids sharing the width-capped transcript card's entry.
    let mut msg = crate::tui::DisplayMessage::todos(payload.to_string());
    msg.title = Some("Pinned todos".to_string());
    super::super::messages::project_cached_message_lines(
        &msg,
        width,
        diff_mode,
        super::super::messages::render_pinned_todos_message,
        project,
    )
}

/// Exact card visibility, without cloning cached lines, spinner animation, or
/// assembling a collapsed/expanded band. Malformed payloads still use the
/// renderer's system-message fallback rather than a separate JSON heuristic.
pub(super) fn pinned_todo_card_has_lines(
    payload: Option<&str>,
    diff_mode: crate::config::DiffDisplayMode,
    width: u16,
) -> bool {
    pinned_todo_card_has_lines_for_mode(
        payload,
        diff_mode,
        width,
        crate::config::config().display.todo_header_mode(),
    )
}

pub(super) fn pinned_todo_card_has_lines_for_mode(
    payload: Option<&str>,
    diff_mode: crate::config::DiffDisplayMode,
    width: u16,
    mode: TodoHeaderMode,
) -> bool {
    match mode {
        TodoHeaderMode::Hidden => false,
        // Minimal always draws exactly one line for a non-empty payload. It does
        // not need the full card projection, which may legitimately be empty for
        // an unparseable payload.
        TodoHeaderMode::Minimal => {
            width >= 16 && payload.is_some_and(|payload| !payload.trim().is_empty())
        }
        TodoHeaderMode::Full => {
            project_pinned_todo_card(payload, diff_mode, width, mode, |lines| !lines.is_empty())
        }
    }
}

/// Pinned todo band for an arbitrary todo payload. Shared by the live chat
/// viewport and the `/resume` preview so both render the band identically.
pub(crate) fn pinned_todo_band_lines_for_payload(
    payload: Option<&str>,
    expanded: bool,
    diff_mode: crate::config::DiffDisplayMode,
    width: u16,
    viewport_height: u16,
) -> (Vec<Line<'static>>, Option<usize>) {
    pinned_todo_band_lines_for_mode(
        payload,
        expanded,
        diff_mode,
        width,
        viewport_height,
        crate::config::config().display.todo_header_mode(),
    )
}

pub(crate) fn pinned_todo_band_lines_for_mode(
    payload: Option<&str>,
    expanded: bool,
    diff_mode: crate::config::DiffDisplayMode,
    width: u16,
    viewport_height: u16,
    mode: TodoHeaderMode,
) -> (Vec<Line<'static>>, Option<usize>) {
    if width < 16 || viewport_height < 3 || mode == TodoHeaderMode::Hidden {
        return (Vec::new(), None);
    }
    if mode == TodoHeaderMode::Minimal {
        let Some(payload) = payload.filter(|payload| !payload.trim().is_empty()) else {
            return (Vec::new(), None);
        };
        return (vec![minimal_todo_header_line(payload)], None);
    }

    let card_lines =
        project_pinned_todo_card(payload, diff_mode, width, mode, <[Line<'static>]>::to_vec);
    PINNED_TODO_LAST_MAX.store(0, std::sync::atomic::Ordering::Relaxed);
    if card_lines.is_empty() {
        return (Vec::new(), None);
    }
    let mut card_lines = card_lines;
    super::super::messages::animate_lane_spinners(&mut card_lines);

    // Band budget: about a third of the viewport.
    let budget = ((viewport_height as usize) / 3).clamp(2, 12);
    let content_budget = budget.max(2);
    let mut lines: Vec<Line<'static>> = Vec::new();
    let has_more = card_lines.len() > content_budget && !expanded;
    let mut more_line = None;
    if has_more {
        let shown = content_budget.saturating_sub(1);
        let hidden = card_lines.len() - shown;
        lines.extend(card_lines.into_iter().take(shown));
        more_line = Some(lines.len());
        lines.push(Line::from(Span::styled(
            format!("  … +{} more (todo)", hidden),
            Style::default().fg(dim_color()),
        )));
    } else {
        if expanded {
            // Expanded bands get a larger budget but stay bounded so a long plan
            // cannot swallow the transcript. Overflow scrolls with the wheel.
            let expanded_budget = ((viewport_height as usize) * 2 / 3).max(content_budget);
            let window = expanded_budget.saturating_sub(1).max(1);
            let total = card_lines.len();
            let max_offset = total.saturating_sub(window);
            let offset = pinned_todo_scroll().min(max_offset);
            PINNED_TODO_LAST_MAX.store(max_offset, std::sync::atomic::Ordering::Relaxed);
            more_line = Some(lines.len());
            let label = if max_offset > 0 {
                format!(
                    "  ▴ Collapse todos ({}-{} of {}, scroll)",
                    offset + 1,
                    (offset + window).min(total),
                    total
                )
            } else {
                "  ▴ Collapse todos".to_string()
            };
            lines.push(Line::from(Span::styled(
                label,
                Style::default().fg(dim_color()),
            )));
            lines.extend(card_lines.into_iter().skip(offset).take(window));
            return (lines, more_line);
        }
        lines.extend(card_lines);
    }
    (lines, more_line)
}

/// Exactly one line of todo state for `todo_header = "minimal"`.
fn minimal_todo_header_line(payload: &str) -> Line<'static> {
    let summary = minimal_todo_summary(payload).unwrap_or_else(|| "Todos".to_string());
    Line::from(vec![
        Span::raw("  "),
        Span::styled(summary, Style::default().fg(dim_color())),
    ])
}

/// Summarize a todo payload as one short phrase: progress counts plus the
/// active (or next pending) task. Returns `None` when the payload is not the
/// expected JSON object, so the caller can fall back to a generic label.
fn minimal_todo_summary(payload: &str) -> Option<String> {
    let value = match serde_json::from_str::<serde_json::Value>(payload) {
        Ok(value) => value,
        Err(_) => return None,
    };
    let todos = value.get("todos")?.as_array()?;
    if todos.is_empty() {
        return Some("Todos: none yet".to_string());
    }
    let status = |todo: &serde_json::Value| -> String {
        todo.get("status")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("pending")
            .to_ascii_lowercase()
    };
    let done = todos
        .iter()
        .filter(|todo| status(todo) == "completed")
        .count();
    let total = todos.len();
    let content = |todo: &serde_json::Value| -> Option<String> {
        let raw = todo.get("content")?.as_str()?.trim();
        if raw.is_empty() {
            return None;
        }
        Some(raw.lines().next().unwrap_or(raw).trim().to_string())
    };
    let active = todos
        .iter()
        .find(|todo| matches!(status(todo).as_str(), "in_progress" | "in-progress"))
        .and_then(content);
    let next = active.is_none().then(|| {
        todos
            .iter()
            .find(|todo| {
                let status = status(todo);
                status != "completed" && status != "cancelled" && status != "canceled"
            })
            .and_then(content)
    });
    let label = |text: String| -> String {
        // The composer owns the width; keep the summary one short phrase.
        let mut text: String = text.chars().take(80).collect();
        if text.chars().count() == 80 {
            text.push('…');
        }
        text
    };
    Some(match (active, next.flatten()) {
        (Some(task), _) => label(format!("Todos {done}/{total} · {task}")),
        (None, Some(task)) => label(format!("Todos {done}/{total} · next: {task}")),
        (None, None) => format!("Todos {done}/{total} done"),
    })
}

pub(crate) fn compute_visible_margins(
    lines: &[Line],
    visible_user_indices: &[usize],
    area: Rect,
    centered: bool,
) -> info_widget::Margins {
    let visible_height = area.height as usize;
    let mut visible_user_cursor = 0usize;

    let mut right_widths = Vec::with_capacity(visible_height);
    let mut left_widths = Vec::with_capacity(visible_height);

    for row in 0..visible_height {
        while visible_user_cursor < visible_user_indices.len()
            && visible_user_indices[visible_user_cursor] < row
        {
            visible_user_cursor += 1;
        }
        let is_user_line = visible_user_cursor < visible_user_indices.len()
            && visible_user_indices[visible_user_cursor] == row;

        if row < lines.len() {
            let mut used = lines[row].width().min(area.width as usize) as u16;
            if is_user_line && area.width > 0 {
                used = used.saturating_add(1).min(area.width);
            }

            // Compute the true free space on each side from the line's *rendered*
            // alignment. This matters even in left-aligned mode: the header lines
            // (`server:`/`client:`/model/version, auth status, mcp list, changelog
            // box, etc.) are always centered regardless of mode, so a centered line
            // of width `used` leaves only ~half the slack on the right. Reporting
            // the full `area.width - used` here would let a right-side info widget
            // overlap the centered header text.
            let total_margin = area.width.saturating_sub(used);
            let default_alignment = if centered {
                Alignment::Center
            } else {
                Alignment::Left
            };
            let effective_alignment = lines[row].alignment.unwrap_or(default_alignment);
            let (left_margin, right_margin) = match effective_alignment {
                Alignment::Left => (0, total_margin),
                Alignment::Center => {
                    let left = total_margin / 2;
                    let right = total_margin.saturating_sub(left);
                    (left, right)
                }
                Alignment::Right => (total_margin, 0),
            };

            if centered {
                left_widths.push(left_margin);
                right_widths.push(right_margin);
            } else {
                // Left-aligned mode never places left-side widgets (content is
                // flush-left), so the left gap is reported as 0; the right gap
                // still respects per-line alignment so widgets clear the header.
                left_widths.push(0);
                right_widths.push(right_margin);
            }
        } else if centered {
            let half = area.width / 2;
            left_widths.push(half);
            right_widths.push(area.width.saturating_sub(half));
        } else {
            left_widths.push(0);
            right_widths.push(area.width);
        }
    }

    info_widget::Margins {
        right_widths,
        left_widths,
        centered,
        ..Default::default()
    }
}

#[cfg(test)]
mod pinned_todo_projection_tests {
    use super::*;

    fn run_isolated(test: &str, pin_todos: bool) -> bool {
        if std::env::var_os("JCODE_PINNED_PROJECTION_CHILD").is_some() {
            assert_eq!(std::env::var("JCODE_TEST_SESSION").as_deref(), Ok("1"));
            assert_eq!(crate::config::config().display.pin_todos, pin_todos);
            return false;
        }
        let home = std::env::temp_dir().join(format!(
            "jcode-pinned-projection-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(
            home.join("config.toml"),
            format!("[display]\npin_todos = {pin_todos}\n"),
        )
        .unwrap();
        let module = module_path!().split_once("::").unwrap().1;
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                &format!("{module}::{test}"),
                "--include-ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("JCODE_HOME", &home)
            .env("JCODE_TEST_SESSION", "1")
            .env("JCODE_PINNED_PROJECTION_CHILD", "1")
            .output()
            .unwrap();
        std::fs::remove_dir_all(home).unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        print!("{stdout}");
        eprint!("{}", String::from_utf8_lossy(&output.stderr));
        assert!(
            output.status.success() && stdout.contains("1 passed;"),
            "isolated {test} failed or did not run"
        );
        true
    }

    fn payload(task_count: usize) -> String {
        serde_json::json!({
            "todos": (0..task_count).map(|i| serde_json::json!({
                "id": format!("task-{i}"),
                "content": format!("Implement and validate scoped task {i}"),
                "status": "in_progress", "priority": "high", "lane": "worker",
                "assigned_to": "session_fox_1700000000"
            })).collect::<Vec<_>>(),
            "plan": {}, "goals": [],
            "agents": [{"session_id": "session_fox_1700000000", "name": "fox", "status": "running", "activity": "Validate scoped task"}]
        }).to_string()
    }

    #[test]
    fn cached_visibility_matches_band_for_payload_width_diff_and_expansion() {
        if run_isolated(
            "cached_visibility_matches_band_for_payload_width_diff_and_expansion",
            true,
        ) {
            return;
        }
        let active = payload(32);
        let empty = payload(0);
        for payload in [
            None,
            Some(""),
            Some("not json"),
            Some("{"),
            Some("null"),
            Some("[]"),
            Some("{}"),
            Some(empty.as_str()),
            Some(active.as_str()),
        ] {
            for width in [0, 1, 15, 16, 17, 40, 120] {
                for diff in [
                    crate::config::DiffDisplayMode::Off,
                    crate::config::DiffDisplayMode::Inline,
                ] {
                    let visible = pinned_todo_card_has_lines(payload, diff, width);
                    for expanded in [false, true] {
                        let (lines, _) =
                            pinned_todo_band_lines_for_payload(payload, expanded, diff, width, 3);
                        assert_eq!(
                            visible,
                            !lines.is_empty(),
                            "payload={payload:?}, width={width}, expanded={expanded}"
                        );
                    }
                    if width < 16 || payload.is_none() || !crate::config::config().display.pin_todos
                    {
                        assert!(!visible);
                    }
                }
            }
        }
    }

    #[test]
    fn cached_visibility_respects_disabled_pinning() {
        if run_isolated("cached_visibility_respects_disabled_pinning", false) {
            return;
        }
        let active = payload(32);
        for width in [0, 15, 16, 120] {
            for payload in [None, Some("not json"), Some(active.as_str())] {
                assert!(!pinned_todo_card_has_lines(
                    payload,
                    crate::config::DiffDisplayMode::Off,
                    width
                ));
                assert!(
                    pinned_todo_band_lines_for_payload(
                        payload,
                        true,
                        crate::config::DiffDisplayMode::Off,
                        width,
                        3
                    )
                    .0
                    .is_empty()
                );
            }
        }
    }

    /// Same binary/profile, cache, payload and width. Alternate order per pair.
    /// This measures the hot visibility query only, not terminal frame time or
    /// allocations. Run serially with an isolated JCODE_HOME and selfdev profile.
    #[test]
    #[ignore = "paired pinned todo cache microbenchmark"]
    fn paired_cached_visibility_benchmark() {
        use std::hint::black_box;
        use std::time::Instant;
        if run_isolated("paired_cached_visibility_benchmark", true) {
            return;
        }
        assert!(crate::config::config().display.pin_todos);
        let iterations = 10_000;
        for count in [1, 32, 128] {
            let payload = payload(count);
            for width in [40, 120] {
                for expanded in [false, true] {
                    let old = || {
                        !pinned_todo_band_lines_for_payload(
                            Some(black_box(payload.as_str())),
                            expanded,
                            crate::config::DiffDisplayMode::Off,
                            width,
                            3,
                        )
                        .0
                        .is_empty()
                    };
                    let new = || {
                        pinned_todo_card_has_lines(
                            Some(black_box(payload.as_str())),
                            crate::config::DiffDisplayMode::Off,
                            width,
                        )
                    };
                    assert_eq!(old(), new());
                    for _ in 0..100 {
                        black_box(old());
                        black_box(new());
                    }
                    let mut old_ns = Vec::new();
                    let mut new_ns = Vec::new();
                    let measure = |f: &dyn Fn() -> bool| {
                        let start = Instant::now();
                        for _ in 0..iterations {
                            black_box(f());
                        }
                        start.elapsed().as_nanos() / iterations
                    };
                    for pair in 0..7 {
                        if pair % 2 == 0 {
                            old_ns.push(measure(&old));
                            new_ns.push(measure(&new));
                        } else {
                            new_ns.push(measure(&new));
                            old_ns.push(measure(&old));
                        }
                    }
                    old_ns.sort_unstable();
                    new_ns.sort_unstable();
                    eprintln!(
                        "pinned_visibility_bench count={count} width={width} expanded={expanded} iterations={iterations} pairs=7 old_median_ns={} new_median_ns={} old_range={:?} new_range={:?}",
                        old_ns[3],
                        new_ns[3],
                        (old_ns[0], old_ns[6]),
                        (new_ns[0], new_ns[6])
                    );
                }
            }
        }
    }

    #[test]
    fn todo_header_full_mode_renders_the_whole_card() {
        let active = payload(4);
        let (lines, _) = pinned_todo_band_lines_for_mode(
            Some(active.as_str()),
            false,
            crate::config::DiffDisplayMode::Off,
            80,
            30,
            TodoHeaderMode::Full,
        );
        assert!(
            lines.len() > 1,
            "full mode must keep the multi-line card, got {} line(s)",
            lines.len()
        );
        assert!(pinned_todo_card_has_lines_for_mode(
            Some(active.as_str()),
            crate::config::DiffDisplayMode::Off,
            80,
            TodoHeaderMode::Full
        ));
    }

    #[test]
    fn todo_header_minimal_mode_is_exactly_one_line() {
        let active = payload(8);
        let (lines, more) = pinned_todo_band_lines_for_mode(
            Some(active.as_str()),
            false,
            crate::config::DiffDisplayMode::Off,
            80,
            30,
            TodoHeaderMode::Minimal,
        );
        assert_eq!(lines.len(), 1, "minimal mode must occupy exactly one row");
        assert!(more.is_none(), "minimal mode has no expand affordance");
        assert!(
            lines[0].to_string().contains("Todos 0/8"),
            "minimal summary should report progress: {:?}",
            lines[0].to_string()
        );
        assert!(pinned_todo_card_has_lines_for_mode(
            Some(active.as_str()),
            crate::config::DiffDisplayMode::Off,
            80,
            TodoHeaderMode::Minimal
        ));
    }

    #[test]
    fn todo_header_hidden_mode_is_zero_lines() {
        let active = payload(8);
        let (lines, more) = pinned_todo_band_lines_for_mode(
            Some(active.as_str()),
            false,
            crate::config::DiffDisplayMode::Off,
            80,
            30,
            TodoHeaderMode::Hidden,
        );
        assert!(lines.is_empty(), "hidden mode must occupy no rows");
        assert!(more.is_none());
        assert!(!pinned_todo_card_has_lines_for_mode(
            Some(active.as_str()),
            crate::config::DiffDisplayMode::Off,
            80,
            TodoHeaderMode::Hidden
        ));
    }
}
