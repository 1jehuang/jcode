use super::*;
use ratatui::backend::TestBackend;
use ratatui::{Terminal, layout::Rect};

/// Render the decision chooser via draw_inline_ui and return the buffer rows.
fn render_decision_chooser(state: &TestState, width: u16, height: u16) -> Vec<String> {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("failed to create test terminal");
    terminal
        .draw(|frame| {
            let area = Rect::new(0, 0, width, height);
            crate::tui::ui::inline_ui::draw_inline_ui(frame, state, area);
        })
        .expect("failed to draw decision chooser");
    let buf = terminal.backend().buffer();
    let mut lines = Vec::with_capacity(height as usize);
    for y in 0..height {
        let mut line = String::with_capacity(width as usize);
        for x in 0..width {
            line.push_str(buf[(x, y)].symbol());
        }
        lines.push(line.trim_end().to_string());
    }
    lines
}

fn decision_state(selected: usize, answer_draft: &str, centered: bool) -> TestState {
    TestState {
        centered_mode: centered,
        pending_decision_state: Some(crate::tui::app::PendingDecision {
            request_id: "decision-1".to_string(),
            question: "Deploy now or stage first?".to_string(),
            options: vec![
                ("Deploy".to_string(), None),
                ("Stage".to_string(), Some("stage first".to_string())),
            ],
            selected,
            answer_draft: answer_draft.to_string(),
        }),
        ..TestState::default()
    }
}

/// decision_truncate_display: every branch. Fits, zero budget, single column,
/// loop truncation with plain chars, and wide chars (emoji) that overflow.
#[test]
fn decision_truncate_display_covers_all_branches() {
    // Fits: unchanged.
    assert_eq!(
        crate::tui::ui::inline_ui::decision_truncate_display_for_test("hello", 10),
        "hello"
    );
    // Zero budget: empty.
    assert_eq!(
        crate::tui::ui::inline_ui::decision_truncate_display_for_test("hello", 0),
        ""
    );
    // One column: just the ellipsis.
    assert_eq!(
        crate::tui::ui::inline_ui::decision_truncate_display_for_test("hello", 1),
        "…"
    );
    // Loop truncation: keep what fits, end with the ellipsis.
    let truncated = crate::tui::ui::inline_ui::decision_truncate_display_for_test("abcdef", 4);
    assert_eq!(truncated, "abc…");
    // Wide chars: a two-column char that does not fit is dropped whole.
    let emoji = crate::tui::ui::inline_ui::decision_truncate_display_for_test("a❓b", 3);
    assert_eq!(emoji, "a…");
}

/// inline_ui_height: the decision branch counts option rows, one row per
/// option detail, the answer rows, and the hint row, capped at 20.
#[test]
fn inline_ui_height_decision_branch_counts_details_and_caps() {
    let two_options_one_detail = decision_state(0, "", false);
    let state = two_options_one_detail;
    assert_eq!(
        crate::tui::ui::inline_ui::inline_ui_height_for_test(&state, 80),
        2 + 1 + 1 + 1 + 3,
        "2 options + 1 detail + answer + hint rows + header + border"
    );

    // Many options with details must cap at the 20-row maximum.
    let tall = TestState {
        pending_decision_state: Some(crate::tui::app::PendingDecision {
            request_id: "decision-2".to_string(),
            question: "q".to_string(),
            options: (0..8)
                .map(|i| (format!("option {i}"), Some("detail".to_string())))
                .collect(),
            selected: 0,
            answer_draft: String::new(),
        }),
        ..Default::default()
    };
    assert_eq!(
        crate::tui::ui::inline_ui::inline_ui_height_for_test(&tall, 80),
        20,
        "the chooser height must cap at 20 rows"
    );
}

/// A tiny area (height or width <= 2) draws nothing and must not panic.
#[test]
fn chooser_in_tiny_area_draws_nothing_without_panicking() {
    let state = decision_state(0, "", false);
    let rows = render_decision_chooser(&state, 2, 2);
    assert!(
        rows.iter().all(|r| r.trim().is_empty()),
        "a 2x2 area must render nothing"
    );
    let rows = render_decision_chooser(&state, 30, 1);
    assert!(
        rows.iter().all(|r| r.trim().is_empty()),
        "a one-row area must render nothing"
    );
}

/// Centered mode offsets the chooser horizontally instead of pinning it left.
/// The chooser is sized to its content, so centered mode only shows an offset
/// in a terminal wider than the chooser itself.
#[test]
fn centered_mode_offsets_the_chooser() {
    // Narrow options keep the chooser well under the 120-col frame.
    let narrow = TestState {
        centered_mode: false,
        pending_decision_state: Some(crate::tui::app::PendingDecision {
            request_id: "decision-3".to_string(),
            question: "Go?".to_string(),
            options: vec![("Yes".to_string(), None), ("No".to_string(), None)],
            selected: 0,
            answer_draft: String::new(),
        }),
        ..Default::default()
    };
    let mut narrow_centered = narrow.clone();
    narrow_centered.centered_mode = true;

    let left_rows = render_decision_chooser(&narrow, 120, 12);
    let centered_rows = render_decision_chooser(&narrow_centered, 120, 12);

    let border_x = |rows: &[String]| {
        rows.iter()
            .map(|r| r.find('╭').unwrap_or(usize::MAX))
            .filter(|x| *x != usize::MAX)
            .min()
            .expect("frame has a border")
    };
    let left_border_x = border_x(&left_rows);
    let centered_border_x = border_x(&centered_rows);
    assert_eq!(left_border_x, 0, "left-aligned chooser starts at column 0");
    assert!(
        centered_border_x > 0,
        "centered mode must offset the chooser right, got {}",
        centered_border_x
    );
}

/// Answer row selected with an empty draft shows the typing hint; with a
/// draft it shows the draft text instead.
#[test]
fn answer_row_hint_renders_only_with_empty_draft() {
    let empty = decision_state(crate::tui::app::PendingDecision::ANSWER_ROW, "", false);
    let rows = render_decision_chooser(&empty, 60, 14);
    let text = rows.join("\n");
    assert!(
        text.contains("Type your own answer"),
        "empty draft must show the typing hint, got: {}",
        text
    );

    let filled = decision_state(
        crate::tui::app::PendingDecision::ANSWER_ROW,
        "ship it friday",
        false,
    );
    let rows = render_decision_chooser(&filled, 60, 14);
    let text = rows.join("\n");
    assert!(
        text.contains("ship it friday"),
        "draft text must render, got: {}",
        text
    );
    assert!(
        !text.contains("Type your own answer"),
        "a filled draft replaces the hint"
    );
}

/// An option row selected (not the answer row) never shows the typing hint.
#[test]
fn option_row_selection_renders_marker_without_hint() {
    let state = decision_state(0, "", false);
    let rows = render_decision_chooser(&state, 60, 14);
    let text = rows.join("\n");
    assert!(text.contains("❯ 1. Deploy"), "selected row shows marker");
    assert!(
        !text.contains("Type your own answer"),
        "hint only renders for the answer row"
    );
}
/// A three-row-tall area fits the border and exactly one content row: the
/// chooser renders the first option and clips the rest, proving the inner
/// region is always non-empty once the height/width <= 2 early return is
/// passed (the border consumes exactly 2 rows/cols and content_width is
/// clamped to at least 1).
#[test]
fn chooser_with_one_inner_row_renders_first_option_without_panicking() {
    let state = decision_state(0, "", false);
    let rows = render_decision_chooser(&state, 40, 3);
    assert!(
        rows.iter().any(|r| r.contains('╭')),
        "the rounded top border must render"
    );
    assert!(
        rows.iter().any(|r| r.contains("1. Deploy")),
        "the single inner row shows the first option"
    );
    assert!(
        !rows.iter().any(|r| r.contains("Your answer")),
        "only one content row fits; the answer row must be clipped"
    );
}

/// A long draft wraps across continuation rows instead of truncating: every
/// word stays visible, continuation rows indent under the draft text, and no
/// answer row ellipsizes.
#[test]
fn long_draft_wraps_across_rows_fully_visible() {
    let state = decision_state(
        crate::tui::app::PendingDecision::ANSWER_ROW,
        "a long typed answer that cannot fit on one single row of the chooser",
        false,
    );
    let rows = render_decision_chooser(&state, 50, 16);
    let text = rows.join("\n");
    for word in [
        "a long typed",
        "answer that",
        "cannot fit",
        "on one single",
        "chooser",
    ] {
        assert!(
            text.contains(word),
            "every draft word must stay visible, missing {word:?}:\n{text}"
        );
    }
    // Nothing needs truncating at this width, so no row may ellipsize:
    // the whole draft is visible across its wrapped rows.
    assert!(
        !text.contains('…'),
        "no row may ellipsize at this width\n{text}"
    );
}

/// Explicit newlines in the draft get their own rendered rows: the first
/// line follows the head, the second indents under it.
#[test]
fn explicit_newline_in_draft_renders_on_its_own_row() {
    let state = decision_state(
        crate::tui::app::PendingDecision::ANSWER_ROW,
        "first line\nsecond line",
        false,
    );
    let rows = render_decision_chooser(&state, 60, 14);
    let head_row = rows
        .iter()
        .find(|r| r.contains("Your answer"))
        .expect("answer head row must render");
    assert!(
        head_row.contains("first line"),
        "first line renders after the head: {head_row:?}"
    );
    let continuation = rows
        .iter()
        .find(|r| r.contains("second line") && *r != head_row)
        .expect("second line must render on its own row");
    assert!(
        continuation.contains("     second line"),
        "continuation rows indent under the draft: {continuation:?}"
    );
}

/// inline_ui_height grows with a selected draft: one row per wrapped line,
/// so the layout reserves exactly what the draw pass renders.
#[test]
fn inline_ui_height_grows_with_selected_draft_and_width() {
    let one_line = decision_state(crate::tui::app::PendingDecision::ANSWER_ROW, "short", false);
    let tall_draft = decision_state(
        crate::tui::app::PendingDecision::ANSWER_ROW,
        "a long typed answer that cannot fit on one single row of the chooser",
        false,
    );
    let short_height = crate::tui::ui::inline_ui::inline_ui_height_for_test(&one_line, 60);
    let tall_height = crate::tui::ui::inline_ui::inline_ui_height_for_test(&tall_draft, 60);
    assert!(
        tall_height > short_height,
        "a wrapped draft must reserve more rows: {tall_height} vs {short_height}"
    );
    // Narrower terminal -> tighter wrap budget -> even more rows.
    let narrow_height = crate::tui::ui::inline_ui::inline_ui_height_for_test(&tall_draft, 30);
    assert!(
        narrow_height > tall_height,
        "a narrower wrap budget must reserve more rows: {narrow_height} vs {tall_height}"
    );
}

/// The hint footer follows the selected row: number/arrow navigation while
/// an option is selected, newline chords while typing an answer.
#[test]
fn hint_follows_selected_row() {
    let option_row = decision_state(0, "", false);
    let rows = render_decision_chooser(&option_row, 70, 14);
    let text = rows.join("\n");
    assert!(
        text.contains("1-9 or ↑↓ + Enter to choose"),
        "option row selected must show the navigation hint: {text}"
    );

    let answer_row = decision_state(crate::tui::app::PendingDecision::ANSWER_ROW, "", false);
    let rows = render_decision_chooser(&answer_row, 70, 14);
    let text = rows.join("\n");
    assert!(
        text.contains("Enter sends · Shift/Alt+Enter newline"),
        "answer row selected must show the newline hint: {text}"
    );
    assert!(
        !text.contains("1-9 or ↑↓"),
        "the navigation hint must not render while typing an answer: {text}"
    );
}

/// Many options overflow the render area: the chooser scrolls a window that
/// keeps the selected row visible instead of clipping it below the fold.
#[test]
fn short_terminal_scrolls_to_keep_selected_row_visible() {
    let mut state = decision_state(7, "", false);
    if let Some(decision) = state.pending_decision_state.as_mut() {
        while decision.options.len() < 10 {
            decision
                .options
                .push((format!("Option {}", decision.options.len() + 1), None));
        }
    }
    // Height 8: border (2) + 6 content rows, fewer than the 10 options +
    // answer + hint the chooser wants to draw.
    let rows = render_decision_chooser(&state, 70, 8);
    let text = rows.join("\n");
    assert!(
        text.contains("❯ 8. Option 8"),
        "the selected row must stay visible in a short terminal: {text}"
    );
}

/// In the same overflow, option 1 (top of the list) scrolls out of view:
/// the window follows the selection, not the list start.
#[test]
fn short_terminal_selected_row_window_overrides_list_start() {
    let mut state = decision_state(7, "", false);
    if let Some(decision) = state.pending_decision_state.as_mut() {
        while decision.options.len() < 10 {
            decision
                .options
                .push((format!("Option {}", decision.options.len() + 1), None));
        }
    }
    let rows = render_decision_chooser(&state, 70, 8);
    let text = rows.join("\n");
    assert!(
        !text.contains("1. Deploy"),
        "the window must scroll past the first option when the selection is low: {text}"
    );
    assert!(
        !text.contains("2. Stage"),
        "the window must scroll past the second option too: {text}"
    );
}

/// The answer row stays reachable in a short terminal: selecting it scrolls
/// the window to its draft even when many options sit above.
#[test]
fn short_terminal_keeps_answer_row_visible() {
    let mut state = decision_state(crate::tui::app::PendingDecision::ANSWER_ROW, "", false);
    if let Some(decision) = state.pending_decision_state.as_mut() {
        while decision.options.len() < 10 {
            decision
                .options
                .push((format!("Option {}", decision.options.len() + 1), None));
        }
    }
    let rows = render_decision_chooser(&state, 70, 8);
    let text = rows.join("\n");
    assert!(
        text.contains("Your answer"),
        "the answer row must stay visible when selected in a short terminal: {text}"
    );
    assert!(
        text.contains("Enter sends · Shift/Alt+Enter newline"),
        "the answer-row hint must stay visible with the row: {text}"
    );
}

/// A selected long draft stays fully visible in a short terminal: the whole
/// selected block, not just its head row, must fit the scrolled window.
#[test]
fn short_terminal_keeps_full_selected_draft_visible() {
    let mut state = decision_state(crate::tui::app::PendingDecision::ANSWER_ROW, "", false);
    if let Some(decision) = state.pending_decision_state.as_mut() {
        while decision.options.len() < 8 {
            decision
                .options
                .push((format!("Option {}", decision.options.len() + 1), None));
        }
        decision.answer_draft =
            "a long typed answer that cannot fit on one single row of the chooser".to_string();
    }
    let rows = render_decision_chooser(&state, 50, 8);
    let text = rows.join("\n");
    assert!(
        text.contains("a long typed"),
        "the draft's head must render in a short terminal: {text}"
    );
    assert!(
        text.contains("chooser"),
        "the draft's tail must render in a short terminal: {text}"
    );
}

/// The chooser never renders taller than the available area: border rows
/// must bookend content even at the extreme overflow.
#[test]
fn short_terminal_window_fits_area_exactly() {
    let mut state = decision_state(0, "", false);
    if let Some(decision) = state.pending_decision_state.as_mut() {
        while decision.options.len() < 10 {
            decision
                .options
                .push((format!("Option {}", decision.options.len() + 1), None));
        }
    }
    let rows = render_decision_chooser(&state, 70, 8);
    assert!(
        rows.iter().any(|r| r.contains('╭')),
        "the rounded top border must render"
    );
    assert!(
        rows.iter().any(|r| r.contains('╰')),
        "the rounded bottom border must render"
    );
    // No row outside the 8-row area: the bottom border row is the last line.
    assert!(
        rows.last().map(|r| r.contains('╰')).unwrap_or(false),
        "the window must end at the area's bottom border"
    );
}

/// A pending decision outranks an interactive picker in inline_ui_state: when
/// a DecisionRequest arrives while a preview picker is open (e.g. /model),
/// the chooser must render above it or the keys route to a hidden widget.
#[test]
fn decision_outranks_interactive_picker_in_inline_ui_state() {
    let mut state = decision_state(0, "", false);
    state.inline_interactive_state = Some(crate::tui::InlineInteractiveState {
        kind: crate::tui::PickerKind::Model,
        filtered: vec![0],
        selected: 0,
        column: 0,
        filter: String::new(),
        preview: true,
        entries: Vec::new(),
    });

    let decision_ref = state.inline_ui_state();
    assert!(
        matches!(
            decision_ref,
            Some(crate::tui::InlineUiStateRef::Decision(_))
        ),
        "a pending decision must outrank an open picker"
    );
}

/// Without a pending decision the picker still wins, so the ranking only
/// changes while a chooser is actually blocking.
#[test]
fn picker_wins_inline_ui_state_without_pending_decision() {
    let state = TestState {
        inline_interactive_state: Some(crate::tui::InlineInteractiveState {
            kind: crate::tui::PickerKind::Model,
            filtered: vec![0],
            selected: 0,
            column: 0,
            filter: String::new(),
            preview: true,
            entries: Vec::new(),
        }),
        ..TestState::default()
    };

    let picker_ref = state.inline_ui_state();
    assert!(
        matches!(
            picker_ref,
            Some(crate::tui::InlineUiStateRef::Interactive(_))
        ),
        "without a decision, the picker must still own the inline slot"
    );
}

/// A huge pasted answer must not collapse the chooser through a u16
/// wraparound: the height saturates at the cap (CHOOSER_MAX_HEIGHT + a few
/// fixed rows), never shrinks below a normal chooser's height.
/// (finding: 65,536 newlines produced an 8-row chooser in an 80-col terminal)
#[test]
fn huge_pasted_answer_saturates_chooser_height_not_shrinks() {
    // 70,000 newlines wraps to far more rows than u16::MAX.
    let huge = "\n".repeat(70_000);
    let state = decision_state(crate::tui::app::PendingDecision::ANSWER_ROW, &huge, false);
    let height = crate::tui::ui::inline_ui::inline_ui_height_for_test(&state, 80);
    assert_eq!(
        height, 20,
        "a huge answer must saturate at the cap, not wrap around u16: got {height}"
    );
}

/// The wrap pass stays linear on huge unbroken pastes: no re-measure of the
/// whole remaining tail per row. This pins the behavior (correct row count,
/// correct widths) with a paste big enough that the old O(n^2) code visibly
/// stalls; the fix keeps output identical.
#[test]
fn wrap_pass_survives_huge_unbroken_paste() {
    let paste = "x".repeat(40_000);
    let rows = crate::tui::ui::inline_ui::wrap_decision_answer_draft(&paste, 40);
    // 40,000 cols / 40 per row = 1,000 rows, no remainder.
    assert_eq!(rows.len(), 1_000, "hard-split must cover every column");
    assert!(rows.iter().all(|r| r.len() == 40), "every row is full");
    // Many newline-separated short rows keep working.
    let multiline = "a\nb\nc";
    let rows = crate::tui::ui::inline_ui::wrap_decision_answer_draft(multiline, 40);
    assert_eq!(
        rows,
        vec!["a".to_string(), "b".to_string(), "c".to_string()]
    );
}
