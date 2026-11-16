//! Manual typing-experience benchmark for resume search: what one keystroke
//! costs on the UI thread, and how many frames show an emptied list while the
//! background search catches up. Run alone with --ignored --nocapture.
use super::*;
use ratatui::{Terminal, backend::TestBackend};
use serde_json::json;
use std::time::{Duration, Instant};

const WORDS: &str = "deployment throughput synchronization performance config ";

/// A history large enough that metadata matching is deferred to the worker,
/// like a real multi-megabyte session index.
pub(super) fn large_history(count: usize, turns: usize) -> SessionPicker {
    let sessions: Vec<_> = (0..count)
        .map(|id| {
            let mut session = performance_tests::session(id, turns);
            let mut index = format!("performance session {id} ");
            while index.len() < 14 * 1024 {
                index.push_str(WORDS);
            }
            session.search_index = index;
            session.first_user_prompt = Some(format!("prompt {id}"));
            session
        })
        .collect();
    let mut picker = SessionPicker::new(sessions);
    for item in &picker.all_sessions {
        picker
            .preview_extras
            .insert(item.id.clone(), PreviewExtras::default());
    }
    picker.set_live_presence_for_test(Vec::new());
    picker
}

pub(super) fn screen_text(terminal: &Terminal<TestBackend>) -> String {
    let buffer = terminal.backend().buffer();
    let mut text = String::new();
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            text.push_str(buffer[(x, y)].symbol());
        }
        text.push('\n');
    }
    text
}

fn ms(values: &[Duration]) -> serde_json::Value {
    let mut sorted: Vec<f64> = values.iter().map(|v| v.as_secs_f64() * 1000.0).collect();
    sorted.sort_by(f64::total_cmp);
    let p = |percent: usize| sorted[(sorted.len() * percent).div_ceil(100).saturating_sub(1)];
    json!({"samples": sorted.len(), "p50": p(50), "p95": p(95), "max": sorted.last()})
}

#[test]
fn editing_coalesces_without_rebuilding_or_starting_search() {
    let mut picker = large_history(12, 1);
    picker.activate_search();
    let rows = picker.visible_sessions.clone();
    let selected = picker.list_state.selected();
    let query = picker.cached_search_query.clone();
    for ch in "deployment thr".chars() {
        picker
            .handle_overlay_key(KeyCode::Char(ch), KeyModifiers::NONE)
            .unwrap();
        assert_eq!(picker.visible_sessions, rows);
        assert_eq!(picker.list_state.selected(), selected);
        assert_eq!(picker.cached_search_query, query);
        assert!(!picker.transcript_search.pending());
    }
    assert!(picker.search_edit_pending);
    assert!(picker.has_pending_preview_work());
    assert_eq!(picker.search_query, "deployment thr");
    // A subsequent poll submits only the final edit, not fourteen searches.
    picker.poll_transcript_search();
    assert!(!picker.search_edit_pending);
    assert_eq!(picker.cached_search_query, "deployment thr");
}

#[test]
fn enter_before_first_poll_never_resumes_a_stale_row() {
    let mut picker = large_history(2, 1);
    picker.activate_search();
    picker
        .handle_overlay_key(KeyCode::Char('z'), KeyModifiers::NONE)
        .unwrap();
    let action = picker
        .handle_overlay_key(KeyCode::Enter, KeyModifiers::NONE)
        .unwrap();
    assert!(matches!(action, OverlayAction::Continue));
    assert!(picker.search_submission_pending());
    assert!(picker.search_edit_pending);
}

#[test]
#[ignore = "manual typing benchmark, run alone with --nocapture --test-threads=1"]
fn benchmark_resume_typing_experience() {
    let mut picker = large_history(700, 8);
    let mut terminal = Terminal::new(TestBackend::new(160, 50)).unwrap();
    terminal.draw(|frame| picker.render(frame)).unwrap();
    picker.activate_search();
    let mut handler = Vec::new();
    let mut key_frame = Vec::new();
    let mut follow_frame = Vec::new();
    let mut frames = 0usize;
    let mut empty_list_frames = 0usize;
    let mut loading_frames = 0usize;
    let mut observe = |terminal: &Terminal<TestBackend>| {
        let text = screen_text(terminal);
        frames += 1;
        if !text.contains("Performance session") {
            empty_list_frames += 1;
        }
        if text.contains("Loading preview") || text.contains("No session selected") {
            loading_frames += 1;
        }
    };
    for round in 0..3 {
        for ch in "deployment thr".chars() {
            let start = Instant::now();
            picker
                .handle_search_key(KeyCode::Char(ch), KeyModifiers::NONE)
                .unwrap();
            handler.push(start.elapsed());
            let start = Instant::now();
            terminal.draw(|frame| picker.render(frame)).unwrap();
            key_frame.push(start.elapsed());
            observe(&terminal);
            // A 64 ms typing cadence observed at the 16 ms poll interval.
            for _ in 0..4 {
                std::thread::sleep(Duration::from_millis(16));
                let start = Instant::now();
                terminal.draw(|frame| picker.render(frame)).unwrap();
                follow_frame.push(start.elapsed());
                observe(&terminal);
            }
        }
        for _ in 0.."deployment thr".len() {
            picker
                .handle_search_key(KeyCode::Backspace, KeyModifiers::NONE)
                .unwrap();
        }
        terminal.draw(|frame| picker.render(frame)).unwrap();
        let _ = round;
    }
    eprintln!(
        "TYPING_BENCH {}",
        json!({
            "sessions": 700, "index_bytes_per_session": 14 * 1024, "terminal": [160, 50],
            "profile": if cfg!(debug_assertions) { "debug" } else { "optimized" },
            "key_handler_ms": ms(&handler), "key_frame_ms": ms(&key_frame),
            "follow_frame_ms": ms(&follow_frame), "frames": frames,
            "empty_list_frames": empty_list_frames, "placeholder_preview_frames": loading_frames,
        })
    );
}
