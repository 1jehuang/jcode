//! Pinned todo bands and viewport margins.

use super::*;

/// Use the exact pinned-card identity and context for both drawing and queries.
fn project_pinned_todo_card<R>(
    payload: Option<&str>,
    diff_mode: crate::config::DiffDisplayMode,
    width: u16,
    active_first: bool,
    project: impl FnOnce(&[Line<'static>]) -> R,
) -> R {
    if width < 16 || !crate::config::config().display.pin_todos {
        return project(&[]);
    }
    let Some(payload) = payload else {
        return project(&[]);
    };
    // Distinct title avoids sharing the width-capped transcript card's entry.
    let mut msg = crate::tui::DisplayMessage::todos(payload.to_string());
    msg.title = Some(
        if active_first {
            "Pinned todos (overflow)"
        } else {
            "Pinned todos"
        }
        .to_string(),
    );
    super::super::messages::project_cached_message_lines(
        &msg,
        width,
        diff_mode,
        |msg, width, mode| super::super::messages::pinned_todo_rows(msg, width, mode, active_first),
        project,
    )
}

/// Exact card visibility, without cloning cached lines, spinner animation, or
/// assembling a collapsed/expanded band. Malformed payloads still use the
/// renderer's system-message fallback rather than a separate JSON heuristic.
#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn pinned_todo_card_has_lines(
    payload: Option<&str>,
    diff_mode: crate::config::DiffDisplayMode,
    width: u16,
) -> bool {
    project_pinned_todo_card(payload, diff_mode, width, false, |lines| !lines.is_empty())
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
    if width < 16 || viewport_height < 3 {
        return (Vec::new(), None);
    }

    PINNED_TODO_LAST_MAX.store(0, std::sync::atomic::Ordering::Relaxed);

    // Band budget: about a third of the viewport.
    let budget = ((viewport_height as usize) / 3).clamp(2, 12);
    let content_budget = budget.max(2);
    let mut lines: Vec<Line<'static>> = Vec::new();
    let normal = project_pinned_todo_card(payload, diff_mode, width, false, |rows| {
        (expanded || rows.len() <= content_budget).then(|| rows.to_vec())
    });
    let has_more = normal.is_none();
    let mut card_lines = normal.unwrap_or_else(|| {
        project_pinned_todo_card(payload, diff_mode, width, true, <[Line<'static>]>::to_vec)
    });
    if card_lines.is_empty() {
        return (Vec::new(), None);
    }
    super::super::messages::animate_lane_spinners(&mut card_lines);
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
}
