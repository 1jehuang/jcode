//! Manual, reproducible measurements of distinct resume interaction surfaces.
//! No terminal emulator, compositor, network, or cold OS page-cache claims.
use super::*;
use crossterm::event::KeyEvent;
use ratatui::{Terminal, backend::TestBackend};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

type TestTerminal = Terminal<TestBackend>;

/// Nearest-rank percentiles. Empty measurements stay null, never fake 0ms.
pub(super) fn distribution_ms(samples: &[Duration]) -> Value {
    if samples.is_empty() {
        return json!({"samples": 0, "p50_ms": null, "p95_ms": null, "p99_ms": null, "max_ms": null, "mean_ms": null, "over_16ms": 0, "over_50ms": 0, "over_100ms": 0, "p99_has_100_samples": false});
    }
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let percentile = |percent: usize| {
        sorted[(sorted.len() * percent).div_ceil(100).saturating_sub(1)].as_secs_f64() * 1000.0
    };
    json!({
        "samples": sorted.len(), "p50_ms": percentile(50), "p95_ms": percentile(95),
        "p99_ms": percentile(99), "max_ms": sorted.last().unwrap().as_secs_f64() * 1000.0,
        "mean_ms": sorted.iter().map(Duration::as_secs_f64).sum::<f64>() * 1000.0 / sorted.len() as f64,
        "over_16ms": sorted.iter().filter(|d| **d > Duration::from_millis(16)).count(),
        "over_50ms": sorted.iter().filter(|d| **d > Duration::from_millis(50)).count(),
        "over_100ms": sorted.iter().filter(|d| **d > Duration::from_millis(100)).count(),
        "p99_has_100_samples": sorted.len() >= 100,
    })
}

pub(super) fn sample_count() -> usize {
    std::env::var("JCODE_RESUME_BENCH_SAMPLES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(20usize)
        .clamp(5, 200)
}

pub(super) fn report(scenario: &str, details: Value, metrics: Value) {
    eprintln!(
        "resume_bench {}",
        json!({
            "schema_version": 1,
            "scenario": scenario,
            "boundary": "in-process handler through ratatui TestBackend draw/flush, not terminal-emulator paint",
            "profile": if cfg!(debug_assertions) { "debug_assertions_enabled" } else { "debug_assertions_disabled" },
            "details": details, "metrics": metrics,
        })
    );
}

fn resident_picker(count: usize, turns: usize) -> SessionPicker {
    let sessions: Vec<_> = (0..count)
        .map(|id| {
            let mut session = performance_tests::session(id, turns);
            for message in &mut session.messages_preview {
                message
                    .content
                    .push_str(&format!("\nSession identity {id}"));
            }
            session
        })
        .collect();
    let mut picker = SessionPicker::new(sessions);
    // Avoid real filesystem/SSH probes: these cases explicitly measure already
    // resident transcripts. Disk-backed readiness is benchmarked separately.
    for item in &picker.all_sessions {
        picker
            .preview_extras
            .insert(item.id.clone(), PreviewExtras::default());
    }
    picker.set_live_presence_for_test(Vec::new());
    picker
}

fn terminal(width: u16, height: u16) -> TestTerminal {
    Terminal::new(TestBackend::new(width, height)).unwrap()
}

fn draw(picker: &mut SessionPicker, terminal: &mut TestTerminal) {
    terminal.draw(|frame| picker.render(frame)).unwrap();
}

fn navigate(picker: &mut SessionPicker, code: KeyCode) -> Duration {
    let start = Instant::now();
    assert!(matches!(
        picker
            .handle_key_event(KeyEvent::new(code, KeyModifiers::NONE))
            .unwrap(),
        OverlayAction::Continue
    ));
    start.elapsed()
}

fn clear_renderer_caches() {
    jcode_tui_messages::clear_message_cache();
    jcode_tui_markdown::clear_highlight_cache();
    // Assistant auxiliary cache is private to ui_prepare. Distinct transcript
    // IDs/content per sample below prevent its message-key reuse. These are
    // cold Jcode renderer caches, NOT cold disk/OS caches.
}

fn unique_transcript(picker: &mut SessionPicker, sample: usize) {
    static GENERATION: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let generation = GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    for session in &mut picker.all_sessions {
        for message in &mut session.messages_preview {
            message.content.push_str(&format!(
                "\nSample identity {sample}, generation {generation}"
            ));
        }
    }
}

#[test]
fn benchmark_distributions_have_honest_empty_and_tail_semantics() {
    let empty = distribution_ms(&[]);
    assert!(empty["p50_ms"].is_null());
    assert_eq!(empty["samples"], 0);
    let samples: Vec<_> = (1..=100).map(Duration::from_millis).collect();
    let stats = distribution_ms(&samples);
    assert_eq!(stats["p50_ms"], 50.0);
    assert_eq!(stats["p95_ms"], 95.0);
    assert_eq!(stats["p99_ms"], 99.0);
    assert_eq!(stats["over_16ms"], 84);
    assert_eq!(stats["over_50ms"], 50);
    assert_eq!(stats["over_100ms"], 0);
    assert_eq!(stats["p99_has_100_samples"], true);
    assert_eq!(distribution_ms(&samples[..5])["p99_has_100_samples"], false);
}

#[test]
fn benchmark_resident_fixtures_never_launch_disk_or_workspace_reads() {
    let mut picker = resident_picker(3, 1);
    let mut terminal = terminal(80, 24);
    for _ in 0..3 {
        draw(&mut picker, &mut terminal);
        assert!(!picker.has_pending_preview_work());
        assert!(
            !picker
                .selected_session()
                .unwrap()
                .messages_preview
                .is_empty()
        );
        navigate(&mut picker, KeyCode::Down);
    }
}

#[test]
#[ignore = "manual resume UX benchmark matrix, run alone with --nocapture --test-threads=1"]
fn benchmark_resume_experience_matrix() {
    let samples = sample_count();
    let mut placeholder = Vec::new();
    for _ in 0..samples {
        let mut picker = SessionPicker::loading();
        let mut terminal = terminal(120, 40);
        let start = Instant::now();
        draw(&mut picker, &mut terminal);
        placeholder.push(start.elapsed());
    }
    report(
        "opening_placeholder",
        json!({"terminal":[120,40],"boundary":"first loading frame only, excludes list scan and command dispatch"}),
        json!({"first_frame":distribution_ms(&placeholder)}),
    );
    // List scalability, independent of transcript rendering and loader cost.
    for count in [100, 1000, 5000] {
        let mut construction = Vec::new();
        let mut navigation = Vec::new();
        let mut frame = Vec::new();
        for _ in 0..samples {
            let sessions: Vec<_> = (0..count)
                .map(|id| performance_tests::session(id, 0))
                .collect();
            let start = Instant::now();
            let mut picker = SessionPicker::new(sessions);
            construction.push(start.elapsed());
            picker.set_live_presence_for_test(Vec::new());
            let mut terminal = terminal(120, 40);
            navigation.push(navigate(&mut picker, KeyCode::Down));
            let start = Instant::now();
            terminal
                .draw(|f| picker.render_session_list_impl(f, f.area(), true))
                .unwrap();
            frame.push(start.elapsed());
        }
        report(
            "list_scale",
            json!({"sessions":count,"terminal":[120,40],"transcript":"omitted","setup_excluded":"fixture creation, terminal allocation"}),
            json!({"picker_construction":distribution_ms(&construction),"key_handler":distribution_ms(&navigation),"list_frame":distribution_ms(&frame)}),
        );
    }
    for (width, height) in [(48, 24), (120, 40), (200, 60)] {
        for turns in [1, 20, 150] {
            let mut cold = Vec::new();
            let mut cold_handler = Vec::new();
            let mut warm = Vec::new();
            let mut idle = Vec::new();
            let mut preview_scroll = Vec::new();
            let mut resize = Vec::new();
            let mut resize_back = Vec::new();
            for sample in 0..samples {
                clear_renderer_caches();
                let mut picker = resident_picker(3, turns);
                unique_transcript(&mut picker, sample);
                let mut terminal = terminal(width, height);
                draw(&mut picker, &mut terminal);
                let start = Instant::now();
                cold_handler.push(navigate(&mut picker, KeyCode::Down));
                draw(&mut picker, &mut terminal);
                cold.push(start.elapsed());
                navigate(&mut picker, KeyCode::Up);
                draw(&mut picker, &mut terminal);
                let start = Instant::now();
                navigate(&mut picker, KeyCode::Down);
                draw(&mut picker, &mut terminal);
                warm.push(start.elapsed());
                let start = Instant::now();
                draw(&mut picker, &mut terminal);
                idle.push(start.elapsed());
                let start = Instant::now();
                picker.scroll_preview_up(5);
                draw(&mut picker, &mut terminal);
                preview_scroll.push(start.elapsed());
                // Terminal recreation is deliberately outside the timed window.
                let mut resized = self::terminal(width.saturating_sub(11).max(1), height);
                let start = Instant::now();
                draw(&mut picker, &mut resized);
                resize.push(start.elapsed());
                let start = Instant::now();
                draw(&mut picker, &mut terminal);
                resize_back.push(start.elapsed());
            }
            report(
                "resident_navigation",
                json!({"sessions":3,"turns_per_session":turns,"terminal":[width,height],"content":"markdown, Rust code, lists, distinct per-session and per-sample prose","cold_definition":"destination prepared-preview miss and distinct content; transcript resident; OS cache not cleared"}),
                json!({"cold_handler_to_frame":distribution_ms(&cold),"cold_key_handler":distribution_ms(&cold_handler),"warm_revisit_handler_to_frame":distribution_ms(&warm),"idle_frame":distribution_ms(&idle),"preview_scroll_to_frame":distribution_ms(&preview_scroll),"resize_frame":distribution_ms(&resize),"resize_back_frame":distribution_ms(&resize_back)}),
            );
        }
    }
    // Retention boundary and rapid navigation: distinguish one frame per input
    // from coalesced events followed by a final frame. Neither simulates key
    // repeat timing or scheduler queue delay, which live diagnostics measure.
    let mut eviction = Vec::new();
    let mut every_event = Vec::new();
    let mut coalesced = Vec::new();
    for _ in 0..samples {
        let mut picker = resident_picker(12, 20);
        let mut terminal = terminal(120, 40);
        for _ in 0..12 {
            draw(&mut picker, &mut terminal);
            navigate(&mut picker, KeyCode::Down);
        }
        for _ in 0..11 {
            navigate(&mut picker, KeyCode::Up);
        }
        let start = Instant::now();
        draw(&mut picker, &mut terminal);
        eviction.push(start.elapsed());
        for per_event in [true, false] {
            let mut picker = resident_picker(12, 20);
            draw(&mut picker, &mut terminal);
            let start = Instant::now();
            for _ in 0..10 {
                navigate(&mut picker, KeyCode::Down);
                if per_event {
                    draw(&mut picker, &mut terminal);
                }
            }
            if !per_event {
                draw(&mut picker, &mut terminal);
            }
            if per_event {
                every_event.push(start.elapsed());
            } else {
                coalesced.push(start.elapsed());
            }
        }
    }
    report(
        "retention_and_bursts",
        json!({"sessions":12,"retained_entry_limit":performance::RETAINED_PREVIEW_ENTRIES,"turns_per_session":20,"terminal":[120,40],"burst_events":10,"cache":"shared renderer caches may be warm; destination prepared previews cold/evicted","boundary":"synchronous handler+frame throughput, not live key-repeat latency"}),
        json!({"evicted_revisit_frame":distribution_ms(&eviction),"ten_events_each_with_frame":distribution_ms(&every_event),"ten_events_one_final_frame":distribution_ms(&coalesced)}),
    );
}

#[test]
#[ignore = "manual resume rich-content benchmark, run alone with --nocapture --test-threads=1"]
fn benchmark_resume_content_matrix() {
    for width in [48, 120] {
        for kind in ["unicode", "large_tool_output", "edit_diff"] {
            let mut first = Vec::new();
            let mut warm_scroll = Vec::new();
            for sample in 0..sample_count() {
                clear_renderer_caches();
                let mut picker = resident_picker(2, 1);
                for session in &mut picker.all_sessions {
                    let msg = &mut session.messages_preview[1];
                    match kind {
                        "unicode" => {
                            msg.content = format!(
                                "sample {sample} session {}\n{}",
                                session.id,
                                "Unicode 界 中文 👩‍💻 e\u{301} and wrapping\n".repeat(200)
                            );
                        }
                        "large_tool_output" => {
                            msg.role = "tool".into();
                            msg.content = (0..800)
                                .map(|i| {
                                    format!(
                                        "{sample} {} output row {i}: result {}\n",
                                        session.id,
                                        "payload ".repeat(10)
                                    )
                                })
                                .collect();
                            msg.tool_data = Some(crate::message::ToolCall {
                                id: format!("tool-{sample}-{}", session.id),
                                name: "bash".into(),
                                input: json!({"command":"synthetic benchmark output", "intent":"benchmark fixture, never executed"}),
                                intent: None,
                                thought_signature: None,
                            });
                        }
                        _ => {
                            msg.role = "tool".into();
                            msg.content = format!(
                                "sample {sample} session {}: replaced src/example.rs",
                                session.id
                            );
                            msg.tool_data = Some(crate::message::ToolCall {
                                id: format!("edit-{sample}-{}", session.id),
                                name: "edit".into(),
                                input: json!({"file_path":"src/example.rs", "old_string":"let previous = 1;\n".repeat(120), "new_string":"let updated = 2;\n".repeat(120)}),
                                intent: None,
                                thought_signature: None,
                            });
                        }
                    }
                }
                let mut terminal = terminal(width, 40);
                let start = Instant::now();
                draw(&mut picker, &mut terminal);
                first.push(start.elapsed());
                let start = Instant::now();
                picker.scroll_preview_up(5);
                draw(&mut picker, &mut terminal);
                warm_scroll.push(start.elapsed());
            }
            report(
                "rich_content",
                json!({"content":kind,"terminal":[width,40],"diff_mode":format!("{:?}",crate::config::config().display.diff_mode),"cache":"prepared preview and message/highlight caches cleared before first frame; OS page cache not cleared"}),
                json!({"first_frame":distribution_ms(&first),"warm_preview_scroll":distribution_ms(&warm_scroll)}),
            );
        }
    }
}
