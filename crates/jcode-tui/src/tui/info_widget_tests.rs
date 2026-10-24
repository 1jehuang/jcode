use super::{
    BackgroundInfo, CacheHitInfo, CacheMissAttribution, GraphEdge, GraphNode, InfoWidgetData,
    Margins, MemoryActivity, MemoryEvent, MemoryEventKind, MemoryInfo, MemoryState, PipelineState,
    StepStatus, SwarmInfo, UsageInfo, UsageProvider, WidgetKind, calculate_placements,
    calculate_widget_height, effective_prompt_tokens, occasional_status_tip,
    render_kv_cache_widget, render_memory_compact, render_memory_widget, render_model_widget,
    render_usage_compact, render_usage_widget, truncate_smart,
};
use crate::protocol::SwarmMemberStatus;
use ratatui::layout::Rect;
use std::time::{Duration, Instant};

fn lines_text_concat(lines: &[ratatui::text::Line<'_>]) -> String {
    lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn widget_selection_has_no_plan_or_todo_variants() {
    let names: Vec<_> = WidgetKind::all_by_priority()
        .iter()
        .map(|kind| kind.as_str())
        .collect();
    assert!(
        !names
            .iter()
            .any(|name| name.contains("todo") || name.contains("plan"))
    );
    for retained in ["overview", "memory", "swarm", "background", "usage", "git"] {
        assert!(
            names.contains(&retained),
            "missing non-plan widget {retained}"
        );
    }
}

#[test]
fn effective_prompt_tokens_handles_split_and_subset_accounting() {
    // Anthropic-style split accounting: `input` is only the uncached remainder,
    // so cache_read pushed beyond input means the true prompt is the sum.
    assert_eq!(
        effective_prompt_tokens("anthropic", 2449, 19499, 684),
        22632
    );
    // OpenAI-style subset accounting: cached tokens are inside `input`.
    assert_eq!(effective_prompt_tokens("openai", 10000, 6000, 0), 10000);
    // No cache telemetry at all behaves like a plain input count.
    assert_eq!(effective_prompt_tokens("openai", 5000, 0, 0), 5000);
}

#[test]
fn cache_hit_ratio_uses_effective_prompt_for_split_providers() {
    // Mirrors a real Anthropic log line where read >> input and the old code
    // clamped the ratio to 100%.
    let cache = CacheHitInfo {
        reported_input_tokens: 2449,
        prompt_tokens: Some(22632),
        read_tokens: 19499,
        creation_tokens: 684,
        ..Default::default()
    };
    // 19499 / (2449 + 19499 + 684) = 0.8616...
    let ratio = cache.hit_ratio().expect("ratio");
    assert!((ratio - 0.8616).abs() < 0.01, "ratio was {ratio}");
}

#[test]
fn truncate_smart_handles_unicode() {
    let s = "eagle running - keep going";
    let out = truncate_smart(s, 15);
    assert_eq!(out, "eagle runnin...");
}

#[test]
fn occasional_status_tip_only_shows_during_part_of_cycle() {
    assert!(occasional_status_tip(60, 5).is_none());
    assert!(occasional_status_tip(60, 27).is_none());
    assert!(occasional_status_tip(60, 28).is_some());
    assert!(occasional_status_tip(60, 39).is_some());
    assert!(occasional_status_tip(60, 40).is_none());
    assert!(occasional_status_tip(60, 89).is_none());
}

#[test]
fn kv_cache_widget_shows_session_hit_ratio() {
    let data = InfoWidgetData {
        cache_hit_info: Some(CacheHitInfo {
            reported_input_tokens: 20_000,
            prompt_tokens: Some(38_000),
            last_prompt_tokens: Some(10_000),
            read_tokens: 15_000,
            creation_tokens: 3_000,
            optimal_input_tokens: 16_667,
            last_reported_input_tokens: Some(10_000),
            last_read_tokens: Some(9_400),
            last_creation_tokens: Some(0),
            last_optimal_input_tokens: Some(9_895),
            miss_attributions: vec![CacheMissAttribution {
                turn_number: 20,
                call_index: 1,
                missed_tokens: 69_000,
                reason: "provider switch".to_string(),
            }],
        }),
        ..Default::default()
    };

    assert!(data.has_data_for(WidgetKind::KvCache));
    let framed = render_kv_cache_widget(&data, Rect::new(0, 0, 40, 5));
    let text = lines_text(&framed.all_lines());

    // Body: the rates line plus one miss row. Headline and totals ride the border.
    assert_eq!(framed.lines.len(), 2);
    assert!(text.contains("KV cache"));
    assert!(text.contains("yield"));
    assert!(text.contains("90%"));
    assert!(text.contains("last "));
    assert!(text.contains("94%"));
    assert!(text.contains("session "));
    assert!(text.contains("39%"));
    assert!(text.contains("69k\n missed"), "{text}");
    assert!(text.contains("20>"));
    assert!(text.contains("69k miss"));
    assert!(text.contains("provider switch"));
}

#[test]
fn kv_cache_without_reuse_baseline_shows_observed_hit_rate() {
    let cache = CacheHitInfo {
        prompt_tokens: Some(100_000),
        read_tokens: 4_000,
        last_prompt_tokens: Some(100_000),
        last_read_tokens: Some(4_000),
        ..Default::default()
    };
    let data = InfoWidgetData {
        cache_hit_info: Some(cache.clone()),
        ..Default::default()
    };
    let framed = render_kv_cache_widget(&data, Rect::new(0, 0, 50, 5));
    let text = lines_text_concat(&framed.all_lines());
    assert!(text.contains("4% hit"), "{text}");
    assert!(!text.contains("priming"), "{text}");
    let compact = lines_text_concat(&[super::render_kv_cache_summary_line(&cache)]);
    assert!(compact.contains("hit 4%"), "{compact}");
    assert!(compact.contains("last 4%"), "{compact}");
    assert!(!compact.contains("priming"), "{compact}");
}

#[test]
fn openai_limit_reached_overrides_nonexhausted_usage_windows() {
    let usage = UsageInfo {
        provider: UsageProvider::OpenAI,
        primary_limit_label: Some("5-hour".to_string()),
        five_hour: 0.18,
        secondary_limit_label: Some("Weekly".to_string()),
        seven_day: 0.03,
        available: true,
        limit_reached: true,
        ..Default::default()
    };
    assert_usage_limit_warning(usage);
}

#[test]
fn openai_limit_reached_without_snapshot_or_windows_is_visible() {
    let usage = UsageInfo {
        provider: UsageProvider::OpenAI,
        limit_reached: true,
        ..Default::default()
    };
    assert!(!usage.available);
    assert_usage_limit_warning(usage);
}

#[test]
fn usage_limit_reached_defaults_to_false() {
    assert!(!UsageInfo::default().limit_reached);
}

fn assert_usage_limit_warning(usage: UsageInfo) {
    assert_eq!(usage.max_usage_pct(), 100);
    let data = InfoWidgetData {
        usage_info: Some(usage.clone()),
        ..Default::default()
    };
    assert!(data.has_data_for(WidgetKind::UsageLimits));
    for display_used in [false, true] {
        let data = InfoWidgetData {
            usage_display_used: display_used,
            ..data.clone()
        };
        let expanded = render_usage_widget(&data, Rect::new(0, 0, 40, 4));
        let compact = render_usage_compact(&usage, 40, display_used);
        let overview = super::render_sections(&data, Rect::new(0, 0, 40, 10), None);
        for text in [
            lines_text(&expanded.all_lines()),
            lines_text(&compact),
            lines_text(&overview),
        ] {
            assert!(text.contains("Usage limit reached"), "{text}");
            assert!(!text.contains('%'), "{text}");
            assert!(!text.contains('▰') && !text.contains('▱'), "{text}");
            assert!(!text.contains("Usage unavailable"), "{text}");
        }
    }
}

#[test]
fn cost_based_usage_widgets_show_price_and_tokens() {
    let usage = UsageInfo {
        provider: UsageProvider::CostBased,
        total_cost: 0.01234,
        input_tokens: 12_345,
        output_tokens: 678,
        available: true,
        ..Default::default()
    };
    let data = InfoWidgetData {
        usage_info: Some(usage.clone()),
        ..Default::default()
    };

    assert!(data.has_data_for(WidgetKind::UsageLimits));

    let expanded_text = lines_text(&render_usage_widget(&data, Rect::new(0, 0, 40, 4)).all_lines());
    assert!(expanded_text.contains("$0.0123"));
    assert!(expanded_text.contains("12.3K in + 678 out"));

    let compact_text = lines_text(&render_usage_compact(&usage, 40, false));
    assert!(compact_text.contains("$0.0123"));
    assert!(compact_text.contains("12.3K in + 678 out"));
}

fn node(kind: &str, label: &str, degree: usize) -> GraphNode {
    GraphNode {
        id: format!("{}:{}", kind, label.replace(' ', "_")),
        label: label.to_string(),
        kind: kind.to_string(),
        is_memory: kind != "tag" && kind != "cluster",
        is_active: true,
        confidence: 0.9,
        degree,
    }
}

fn edge(source: usize, target: usize, kind: &str) -> GraphEdge {
    GraphEdge {
        source,
        target,
        kind: kind.to_string(),
    }
}

fn lines_text(lines: &[ratatui::text::Line<'_>]) -> String {
    lines
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn memory_widget_hides_sidecar_model_when_idle() {
    let info = MemoryInfo {
        total_count: 3,
        project_count: 2,
        global_count: 1,
        sidecar_available: true,
        sidecar_model: Some("openai · gpt-5.3-codex-spark".to_string()),
        ..Default::default()
    };
    let data = InfoWidgetData {
        memory_info: Some(info),
        ..Default::default()
    };

    let text = render_memory_widget(&data, Rect::new(0, 0, 40, 5))
        .all_lines()
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect::<Vec<_>>()
        .join("\n")
        .to_lowercase();

    assert!(text.contains("3 memories"));
    assert!(!text.contains("model:"));
    assert!(!text.contains("gpt-5.3"));
    assert!(text.contains("3 memories"));
}

#[test]
fn memory_widget_renders_current_cycle_activity() {
    let now = Instant::now();
    let mut pipeline = PipelineState::new();
    pipeline.search = StepStatus::Done;
    pipeline.verify = StepStatus::Running;
    pipeline.verify_progress = Some((1, 3));

    let data = InfoWidgetData {
        memory_info: Some(MemoryInfo {
            total_count: 7,
            project_count: 4,
            global_count: 3,
            sidecar_model: Some("openai · gpt-5.3-codex-spark".to_string()),
            activity: Some(MemoryActivity {
                state: MemoryState::SidecarChecking { count: 3 },
                state_since: now - Duration::from_secs(12),
                pipeline: Some(pipeline),
                recent_events: vec![
                    MemoryEvent {
                        kind: MemoryEventKind::MemoryInjected {
                            count: 2,
                            prompt_chars: 318,
                            age_ms: 44,
                            preview: "prefers terse answers".to_string(),
                            items: Vec::new(),
                        },
                        timestamp: now - Duration::from_secs(11),
                        detail: None,
                    },
                    MemoryEvent {
                        kind: MemoryEventKind::EmbeddingComplete {
                            latency_ms: 71,
                            hits: 9,
                        },
                        timestamp: now - Duration::from_secs(12),
                        detail: None,
                    },
                ],
            }),
            graph_nodes: vec![node("fact", "release build", 2), node("tag", "rust", 1)],
            graph_edges: vec![edge(0, 1, "has_tag")],
            ..Default::default()
        }),
        ..Default::default()
    };

    let text = render_memory_widget(&data, Rect::new(0, 0, 40, 8))
        .all_lines()
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect::<Vec<_>>()
        .join("\n")
        .to_lowercase();

    assert!(text.contains("7 memories"));
    assert!(text.contains("load memories"));
    assert!(text.contains("jev relevance"));
    assert!(text.contains("1/3"));
    assert!(text.contains("inject context"));
    assert!(text.contains("update memory"));
    assert!(text.contains("now:"));
    assert!(text.contains("jev relevance: 3"));
    assert!(!text.contains("model:"));
    assert!(!text.contains("gpt-5.3"));
    assert!(!text.contains("4 project"));
    assert!(!text.contains("3 global"));
}

#[test]
fn memory_widget_marks_completed_pipeline_even_when_state_is_idle() {
    let now = Instant::now();
    let mut pipeline = PipelineState::new();
    pipeline.search = StepStatus::Done;
    pipeline.verify = StepStatus::Done;
    pipeline.inject = StepStatus::Done;
    pipeline.maintain = StepStatus::Done;

    let data = InfoWidgetData {
        memory_info: Some(MemoryInfo {
            sidecar_model: Some("openai · gpt-5.3-codex-spark".to_string()),
            activity: Some(MemoryActivity {
                state: MemoryState::Idle,
                state_since: now - Duration::from_secs(4),
                pipeline: Some(pipeline),
                recent_events: vec![MemoryEvent {
                    kind: MemoryEventKind::MemoryInjected {
                        count: 1,
                        prompt_chars: 42,
                        age_ms: 12,
                        preview: "prefers terse answers".to_string(),
                        items: Vec::new(),
                    },
                    timestamp: now - Duration::from_secs(3),
                    detail: None,
                }],
            }),
            ..Default::default()
        }),
        ..Default::default()
    };

    let text = render_memory_widget(&data, Rect::new(0, 0, 40, 4))
        .all_lines()
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect::<Vec<_>>()
        .join("\n")
        .to_lowercase();

    assert!(text.contains("done"));
    assert!(text.contains("last:"));
}

#[test]
fn memory_widget_does_not_stay_done_after_idle_settles() {
    let now = Instant::now();
    let mut pipeline = PipelineState::new();
    pipeline.search = StepStatus::Done;
    pipeline.verify = StepStatus::Done;
    pipeline.inject = StepStatus::Done;
    pipeline.maintain = StepStatus::Done;

    let data = InfoWidgetData {
        memory_info: Some(MemoryInfo {
            total_count: 128,
            activity: Some(MemoryActivity {
                state: MemoryState::Idle,
                state_since: now - Duration::from_secs(12),
                pipeline: Some(pipeline),
                recent_events: vec![MemoryEvent {
                    kind: MemoryEventKind::MemoryInjected {
                        count: 1,
                        prompt_chars: 42,
                        age_ms: 12,
                        preview: "prefers terse answers".to_string(),
                        items: Vec::new(),
                    },
                    timestamp: now - Duration::from_secs(11),
                    detail: None,
                }],
            }),
            ..Default::default()
        }),
        ..Default::default()
    };

    let text = render_memory_widget(&data, Rect::new(0, 0, 50, 6))
        .all_lines()
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect::<Vec<_>>()
        .join("\n")
        .to_lowercase();

    assert!(text.contains("128 memories"), "{text}");
    assert!(!text.contains("done"), "{text}");
    assert!(!text.contains("idle"), "{text}");
    assert!(!text.contains("trace:"), "{text}");
}

#[test]
fn memory_widget_never_renders_uppercase_state_badges() {
    let data = InfoWidgetData {
        memory_info: Some(MemoryInfo {
            total_count: 128,
            activity: Some(MemoryActivity {
                state: MemoryState::SidecarChecking { count: 3 },
                state_since: Instant::now(),
                pipeline: None,
                recent_events: Vec::new(),
            }),
            ..Default::default()
        }),
        ..Default::default()
    };

    let text = render_memory_widget(&data, Rect::new(0, 0, 40, 8))
        .all_lines()
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect::<String>();

    assert!(text.contains("128 memories"), "{text}");
    for badge in [
        "IDLE", "SEARCH", "JEV", "READY", "INJECT", "SAVE", "UPDATE", "TOOL", "DONE", "FAILED",
        "DISABLED",
    ] {
        assert!(!text.contains(badge), "unexpected badge {badge}: {text}");
    }
}

#[test]
fn memory_widget_uses_distinct_trace_label_when_idle() {
    let now = Instant::now();
    let mut pipeline = PipelineState::new();
    pipeline.search = StepStatus::Done;
    pipeline.verify = StepStatus::Done;
    pipeline.inject = StepStatus::Done;
    pipeline.maintain = StepStatus::Done;

    let data = InfoWidgetData {
        memory_info: Some(MemoryInfo {
            sidecar_model: Some("openai · gpt-5.3-codex-spark".to_string()),
            activity: Some(MemoryActivity {
                state: MemoryState::Idle,
                state_since: now - Duration::from_secs(4),
                pipeline: Some(pipeline),
                recent_events: vec![MemoryEvent {
                    kind: MemoryEventKind::MemoryInjected {
                        count: 1,
                        prompt_chars: 42,
                        age_ms: 12,
                        preview: "prefers terse answers".to_string(),
                        items: Vec::new(),
                    },
                    timestamp: now - Duration::from_secs(3),
                    detail: None,
                }],
            }),
            ..Default::default()
        }),
        ..Default::default()
    };

    let text = render_memory_widget(&data, Rect::new(0, 0, 60, 8))
        .all_lines()
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect::<Vec<_>>()
        .join("\n")
        .to_lowercase();

    assert_eq!(text.matches("last:").count(), 1, "{text}");
    assert!(text.contains("trace:"), "{text}");
}

#[test]
fn memory_compact_does_not_show_model() {
    let lines = render_memory_compact(
        &MemoryInfo {
            sidecar_model: Some("openai · gpt-5.3-codex-spark".to_string()),
            ..Default::default()
        },
        30,
    );

    let text = lines
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect::<Vec<_>>()
        .join("\n")
        .to_lowercase();

    assert!(!text.contains("gpt-5.3"), "{text}");
    assert!(!text.contains("codex-spark"), "{text}");
}

#[test]
fn memory_compact_shows_memory_count_before_status() {
    let lines = render_memory_compact(
        &MemoryInfo {
            total_count: 128,
            activity: Some(MemoryActivity {
                state: MemoryState::Idle,
                state_since: Instant::now() - Duration::from_secs(8),
                pipeline: None,
                recent_events: Vec::new(),
            }),
            ..Default::default()
        },
        30,
    );

    let text = lines
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect::<Vec<_>>()
        .join("\n")
        .to_lowercase();

    assert!(text.contains("128 memories"), "{text}");
    assert!(!text.contains("idle"), "{text}");
    assert!(!text.contains(" · "), "{text}");
    assert!(!text.contains("memory ·"), "{text}");
}

#[test]
fn memory_widget_is_hidden_when_disabled() {
    let data = InfoWidgetData {
        memory_info: Some(MemoryInfo {
            total_count: 12,
            project_count: 8,
            global_count: 4,
            disabled: true,
            ..Default::default()
        }),
        ..Default::default()
    };

    assert!(render_memory_widget(&data, Rect::new(0, 0, 40, 5)).is_empty());
    assert!(render_memory_compact(data.memory_info.as_ref().unwrap(), 40).is_empty());
    assert!(!data.has_data_for(WidgetKind::MemoryActivity));
}

#[test]
fn memory_widget_shows_option_a_steps_without_pipeline_object() {
    let data = InfoWidgetData {
        memory_info: Some(MemoryInfo {
            sidecar_model: Some("openai · gpt-5.3-codex-spark".to_string()),
            activity: Some(MemoryActivity {
                state: MemoryState::SidecarChecking { count: 3 },
                state_since: Instant::now(),
                pipeline: None,
                recent_events: Vec::new(),
            }),
            ..Default::default()
        }),
        ..Default::default()
    };

    let text = render_memory_widget(&data, Rect::new(0, 0, 40, 8))
        .all_lines()
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect::<Vec<_>>()
        .join("\n")
        .to_lowercase();

    assert!(text.contains("load memories"), "{text}");
    assert!(text.contains("jev relevance"), "{text}");
    assert!(text.contains("inject context"), "{text}");
    assert!(text.contains("update memory"), "{text}");
    assert!(text.contains("jev relevance: 3"), "{text}");
}

#[test]
fn memory_activity_priority_is_elevated_while_processing() {
    let mut idle_data = InfoWidgetData {
        memory_info: Some(MemoryInfo {
            total_count: 2,
            activity: Some(MemoryActivity {
                state: MemoryState::Idle,
                state_since: Instant::now(),
                pipeline: None,
                recent_events: Vec::new(),
            }),
            ..Default::default()
        }),
        ..Default::default()
    };

    assert_eq!(
        idle_data.effective_priority(WidgetKind::MemoryActivity),
        WidgetKind::MemoryActivity.priority()
    );

    idle_data.memory_info.as_mut().unwrap().activity = Some(MemoryActivity {
        state: MemoryState::Embedding,
        state_since: Instant::now(),
        pipeline: None,
        recent_events: Vec::new(),
    });

    assert_eq!(idle_data.effective_priority(WidgetKind::MemoryActivity), 0);
}

#[test]
fn contextual_subgraph_prefers_memory_hub() {
    let mut nodes = vec![
        node("fact", "core build flow", 6),
        node("preference", "use cargo test", 4),
        node("tag", "rust", 5),
        node("tag", "testing", 3),
        node("fact", "docs in readme", 1),
    ];
    nodes[0].is_active = true;
    nodes[0].confidence = 0.95;

    let info = MemoryInfo {
        total_count: 5,
        graph_nodes: nodes,
        graph_edges: vec![
            edge(0, 1, "relates_to"),
            edge(0, 2, "has_tag"),
            edge(1, 3, "has_tag"),
            edge(4, 2, "has_tag"),
        ],
        ..Default::default()
    };

    let subgraph = super::select_contextual_subgraph(&info, 3, 6).expect("subgraph");
    assert_eq!(subgraph.nodes.len(), 3);
    assert!(
        subgraph
            .nodes
            .iter()
            .any(|n| n.label.contains("core build flow"))
    );
}

#[test]
fn overview_requires_multiple_sections() {
    // The Overview is a single always-visible panel: any section is enough,
    // no multi-section join threshold. The selected model + effort row (🏠)
    // is a runtime section on its own.
    let identity_only = InfoWidgetData {
        model: Some("gpt-test".to_string()),
        queue_mode: Some(true),
        ..Default::default()
    };
    assert!(identity_only.has_data_for(WidgetKind::Overview));

    // A lone non-status-line section still places the overview.
    let one_section = InfoWidgetData {
        queue_mode: Some(true),
        ..Default::default()
    };
    assert!(one_section.has_data_for(WidgetKind::Overview));

    // Truly empty data has nothing to render: the overview stays out of the
    // layout instead of placing an empty frame.
    let no_sections = InfoWidgetData::default();
    assert!(!no_sections.has_data_for(WidgetKind::Overview));

    let two_sections = InfoWidgetData {
        session_name: Some("sauropod".to_string()),
        queue_mode: Some(true),
        ..Default::default()
    };
    assert!(two_sections.has_data_for(WidgetKind::Overview));

    // A disabled memory feature is not an overview section: `disabled` means
    // no recall/extraction activity, so there is no detail to join.
    let disabled_memory = InfoWidgetData {
        memory_info: Some(MemoryInfo {
            disabled: true,
            ..Default::default()
        }),
        ..Default::default()
    };
    assert!(!disabled_memory.has_data_for(WidgetKind::Overview));
    assert!(!disabled_memory.has_data_for(WidgetKind::MemoryActivity));

    // End-to-end through the real public render path: place the overview
    // dock with calculate_placements and render_all into a TestBackend,
    // then assert at the cell level what the user would actually see.
    // A disabled memory feature must not paint a 🧠 row; an enabled one
    // must.
    {
        let _lock = crate::tui::ui::render_state_test_lock();
        // Stateless placement (`calculate_placements_anchored`) instead of the
        // shared-state `calculate_placements`: parallel tests mutate the
        // global widget state, which would blank this draw non-deterministically.
        let area = Rect::new(0, 0, 38, 24);
        let margins = super::Margins {
            right_widths: vec![38; 24],
            ..Default::default()
        };

        let draw_overview = |data: &InfoWidgetData| -> String {
            let outcome = super::super::info_widget_layout::calculate_placements_anchored(
                area,
                &margins,
                data,
                true,
                &[],
            );
            let backend = ratatui::backend::TestBackend::new(38, 24);
            let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
            terminal
                .draw(|frame| {
                    super::render_all(frame, &outcome.visible, data);
                })
                .expect("overview draw should not panic");
            let buf = terminal.backend().buffer();
            (0..buf.area.height)
                .map(|y| {
                    (0..buf.area.width)
                        .map(|x| buf[(x, y)].symbol().to_string())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n")
        };

        let mut disabled = disabled_memory.clone();
        disabled.queue_mode = Some(true);
        let rendered = draw_overview(&disabled);
        assert!(
            !rendered.contains('🧠'),
            "disabled memory must not render a 🧠 row in the overview, got: {rendered:?}"
        );

        let mut enabled = disabled_memory.clone();
        enabled.queue_mode = Some(true);
        enabled.memory_info = Some(MemoryInfo {
            total_count: 4,
            ..Default::default()
        });
        let rendered = draw_overview(&enabled);
        assert!(
            rendered.contains('🧠'),
            "enabled memory must render its 🧠 row in the overview, got: {rendered:?}"
        );
    }
}

#[test]
fn overview_widget_is_placed_when_space_allows() {
    {
        let mut guard = super::get_or_init_state();
        if let Some(state) = guard.as_mut() {
            state.enabled = true;
            state.placements.clear();
            state.anchors.clear();
            state.widget_states.clear();
        }
    }

    let data = InfoWidgetData {
        session_name: Some("sauropod".to_string()),
        queue_mode: Some(true),
        ..Default::default()
    };
    let margins = Margins {
        right_widths: vec![40; 20],
        left_widths: Vec::new(),
        centered: false,
        ..Default::default()
    };
    let placements = calculate_placements(Rect::new(0, 0, 80, 20), &margins, &data);
    assert!(
        placements.iter().any(|p| p.kind == WidgetKind::Overview),
        "expected overview widget placement"
    );
}

#[test]
fn workspace_widget_has_high_priority_when_enabled() {
    {
        let mut guard = super::get_or_init_state();
        if let Some(state) = guard.as_mut() {
            state.enabled = true;
            state.placements.clear();
            state.anchors.clear();
            state.widget_states.clear();
        }
    }

    let data = InfoWidgetData {
        workspace_rows: vec![crate::tui::workspace_map::VisibleWorkspaceRow {
            workspace: 0,
            is_current: true,
            focused_index: Some(0),
            sessions: vec![crate::tui::workspace_map::WorkspaceSessionTile::new("fox")],
        }],
        session_name: Some("sauropod".to_string()),
        queue_mode: Some(true),
        ..Default::default()
    };

    let available = data.available_widgets();
    assert_eq!(available.first(), Some(&WidgetKind::WorkspaceMap));

    let margins = Margins {
        right_widths: vec![40; 20],
        left_widths: Vec::new(),
        centered: false,
        ..Default::default()
    };
    let placements = calculate_placements(Rect::new(0, 0, 80, 20), &margins, &data);
    assert_eq!(
        placements.first().map(|p| p.kind),
        Some(WidgetKind::WorkspaceMap)
    );
}

#[test]
fn model_widget_renders_connection_type() {
    let data = InfoWidgetData {
        model: Some("gpt-5.3-codex".to_string()),
        provider_name: Some("openai".to_string()),
        connection_type: Some("websocket".to_string()),
        ..Default::default()
    };
    let text = render_model_widget(&data, Rect::new(0, 0, 40, 10))
        .all_lines()
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect::<Vec<_>>()
        .join("\n")
        .to_lowercase();
    assert!(text.contains("websocket"));
}

fn managed_member(id: &str, status: &str, role: Option<&str>) -> SwarmMemberStatus {
    SwarmMemberStatus {
        session_id: id.to_string(),
        friendly_name: Some(id.to_string()),
        status: status.to_string(),
        detail: None,
        task_label: None,
        role: role.map(str::to_string),
        is_headless: Some(true),
        live_attachments: None,
        status_age_secs: Some(3),
        output_tail: Some("streaming some work".to_string()),
        report_back_to_session_id: Some("parent".to_string()),
        todo_progress: Some((2, 5)),
        todo_items: Vec::new(),
        runtime: crate::protocol::SwarmMemberRuntime::default(),
    }
}

/// With managed members present, the SwarmStatus widget switches to compact
/// mode: agents tally without plan progress, and has_data_for admits the
/// widget into layout.
#[test]
fn swarm_widget_dock_mode_lists_managed_agents() {
    let data = InfoWidgetData {
        swarm_info: Some(SwarmInfo {
            managed_members: vec![
                managed_member("researcher", "running", Some("coordinator")),
                managed_member("reviewer", "completed", None),
            ],
            selected: 0,
            ..Default::default()
        }),
        ..Default::default()
    };
    assert!(data.has_data_for(WidgetKind::SwarmStatus));
    // Managing agents bumps the dock's effective priority near the top.
    assert!(data.effective_priority(WidgetKind::SwarmStatus) < WidgetKind::SwarmStatus.priority());

    let framed = super::render_swarm_widget(&data, Rect::new(0, 0, 34, 10));
    let title = lines_text_concat(framed.title.as_slice());
    assert!(title.contains("1/2 active"), "got: {title}");
    let footer = lines_text_concat(framed.footer_right.as_slice());
    assert!(footer.is_empty(), "plan meter must not render: {footer}");
    assert!(!lines_text_concat(&framed.lines).contains("2/5"));
    // One row per agent: working agents before finished ones.
    assert_eq!(framed.lines.len(), 2, "one row per agent");
    let rows: Vec<String> = framed
        .lines
        .iter()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect();
    assert!(rows[0].contains("★ researcher"), "{rows:?}");
    assert!(
        !rows[0].contains("2/5"),
        "todo progress must not appear on the row: {rows:?}"
    );
    assert!(rows[1].starts_with("✓ reviewer"), "{rows:?}");
    for line in &framed.lines {
        assert!(line.width() <= 34, "row too wide: {rows:?}");
    }
    let h = calculate_widget_height(WidgetKind::SwarmStatus, &data, 36, 20);
    assert_eq!(h, 4, "2 agent rows + 2 border: {h}");
}

#[test]
fn swarm_dock_surfaces_attention_and_collapses_overflow() {
    let mut failed = managed_member("reviewer", "failed", None);
    failed.detail = Some("cargo test failed".to_string());
    let mut members: Vec<_> = (0..8)
        .map(|i| managed_member(&format!("w{i}"), "completed", None))
        .collect();
    members.push(failed);
    let data = InfoWidgetData {
        swarm_info: Some(SwarmInfo {
            managed_members: members,
            ..Default::default()
        }),
        ..Default::default()
    };
    let framed = super::render_swarm_widget(&data, Rect::new(0, 0, 40, 20));
    let first: String = framed.lines[0]
        .spans
        .iter()
        .map(|s| s.content.as_ref())
        .collect();
    assert!(
        first.contains("reviewer") && first.contains("cargo test failed"),
        "{first}"
    );
    assert!(lines_text(framed.title_right.as_slice()).contains("⚠ 1"));
    assert_eq!(framed.lines.len(), 6, "rows capped");
    assert!(lines_text(framed.footer.as_slice()).contains("+3 more"));
    // Tiny sizes never panic or overflow.
    for (w, h) in [(0, 0), (1, 1), (8, 2), (14, 3)] {
        let f = super::render_swarm_widget(&data, Rect::new(0, 0, w, h));
        for line in &f.lines {
            assert!(line.width() <= usize::from(w), "w={w}");
        }
    }
}

#[test]
fn swarm_dock_shows_health_without_repeating_todo_activity() {
    let mut worker = managed_member("blowfish", "running", None);
    worker.detail = Some("You work across the toolchain and compiler runtime".to_string());
    worker.todo_items = vec![
        crate::protocol::SwarmTodoItem {
            content: "Measure baseline".to_string(),
            status: "completed".to_string(),
            tool_intents: Vec::new(),
        },
        crate::protocol::SwarmTodoItem {
            content: "Build the per-step ranking script for rustc passes".to_string(),
            status: "in_progress".to_string(),
            tool_intents: Vec::new(),
        },
    ];
    let data = InfoWidgetData {
        swarm_info: Some(SwarmInfo {
            managed_members: vec![worker],
            ..Default::default()
        }),
        ..Default::default()
    };
    let width = 40u16;
    let framed = super::render_swarm_widget(&data, Rect::new(0, 0, width, 10));
    let title = lines_text_concat(framed.title.as_slice());
    assert!(title.starts_with("🐝 1/1 active"), "icon only: {title}");
    assert!(!title.contains("Swarm"), "no spelled-out label: {title}");
    let row: String = framed.lines[0]
        .spans
        .iter()
        .map(|s| s.content.as_ref())
        .collect();
    assert!(
        row.contains(crate::id::session_icon("blowfish")),
        "agent icon: {row}"
    );
    assert!(
        !row.contains("blowfish"),
        "no spelled-out agent name: {row}"
    );
    assert!(row.contains("running"), "lifecycle health: {row}");
    assert!(
        !row.contains("You work across"),
        "no duplicate activity: {row}"
    );
    assert!(
        !row.contains("Build the per-step"),
        "todo must stay out: {row}"
    );
    assert!(!row.contains("..."), "single-char ellipsis: {row}");
    // The compact health row still respects the available display cells.
    assert!(
        unicode_width::UnicodeWidthStr::width(row.as_str()) <= usize::from(width),
        "{row}"
    );
}

#[test]
fn swarm_dock_preserves_messages_blockers_and_quiet_health() {
    for (status, detail, activity_age, expected) in [
        (
            "running",
            "DM from fox: tests ready",
            0,
            "DM from fox: tests ready",
        ),
        (
            "running",
            "Cross-swarm DM from fox: hello",
            0,
            "Cross-swarm DM from fox: hello",
        ),
        (
            "blocked",
            "Need approval to proceed",
            0,
            "Need approval to proceed",
        ),
        ("failed", "Tests failed", 0, "Tests failed"),
        ("running", "Implement parser", 180, "quiet 3m"),
        ("completed", "Implement parser", 180, "completed"),
    ] {
        let mut worker = managed_member("fox", status, None);
        worker.detail = Some(detail.to_string());
        worker.runtime.activity_age_secs = Some(activity_age);
        let data = InfoWidgetData {
            swarm_info: Some(SwarmInfo {
                managed_members: vec![worker],
                ..Default::default()
            }),
            ..Default::default()
        };
        let framed = super::render_swarm_widget(&data, Rect::new(0, 0, 80, 5));
        let row = lines_text_concat(&framed.lines);
        assert!(row.contains(expected), "{status}: {row}");
        assert!(!row.contains("streaming some work"), "{row}");
        for width in [0, 1, 8, 14, 24] {
            let narrow = super::render_swarm_widget(&data, Rect::new(0, 0, width, 5));
            assert!(
                narrow
                    .lines
                    .iter()
                    .all(|line| line.width() <= width as usize)
            );
        }
    }
}

/// Without managed members the legacy session-list rendering is preserved and
/// the widget stays out of layout (has_data_for is false).
#[test]
fn swarm_widget_without_managed_agents_stays_hidden_from_layout() {
    let data = InfoWidgetData {
        swarm_info: Some(SwarmInfo {
            session_count: 4,
            session_names: vec!["alpha".to_string()],
            ..Default::default()
        }),
        ..Default::default()
    };
    assert!(!data.has_data_for(WidgetKind::SwarmStatus));
    assert_eq!(
        calculate_widget_height(WidgetKind::SwarmStatus, &data, 34, 20),
        0
    );
}

#[test]
fn swarm_widget_renders_member_roles_and_details() {
    let data = InfoWidgetData {
        swarm_info: Some(SwarmInfo {
            session_count: 3,
            client_count: Some(1),
            members: vec![
                SwarmMemberStatus {
                    session_id: "coord-12345678".to_string(),
                    friendly_name: Some("coord".to_string()),
                    status: "running".to_string(),
                    detail: Some("orchestrating patch".to_string()),
                    task_label: None,
                    role: Some("coordinator".to_string()),
                    is_headless: None,
                    live_attachments: None,
                    status_age_secs: None,
                    output_tail: None,
                    report_back_to_session_id: None,
                    todo_progress: None,
                    todo_items: Vec::new(),
                    runtime: crate::protocol::SwarmMemberRuntime::default(),
                },
                SwarmMemberStatus {
                    session_id: "tree-12345678".to_string(),
                    friendly_name: Some("trees".to_string()),
                    status: "ready".to_string(),
                    detail: Some("worktree synced".to_string()),
                    task_label: None,
                    role: Some("agent".to_string()),
                    is_headless: None,
                    live_attachments: None,
                    status_age_secs: None,
                    output_tail: None,
                    report_back_to_session_id: None,
                    todo_progress: None,
                    todo_items: Vec::new(),
                    runtime: crate::protocol::SwarmMemberRuntime::default(),
                },
            ],
            ..Default::default()
        }),
        ..Default::default()
    };

    let text = lines_text(&super::render_swarm_widget(&data, Rect::new(0, 0, 80, 4)).all_lines());

    assert!(text.contains("3s"), "got: {text}");
    assert!(text.contains("1 client"), "got: {text}");
    assert!(text.contains("★"), "got: {text}");
    assert!(
        text.contains("coord running - orchestrating patch"),
        "got: {text}"
    );
    assert!(
        text.contains("trees ready - worktree synced"),
        "got: {text}"
    );
}

#[test]
fn swarm_widget_handles_empty_swarm_and_zero_area_without_panic() {
    // No swarm info at all: renders nothing.
    let data = InfoWidgetData::default();
    assert!(super::render_swarm_widget(&data, Rect::new(0, 0, 40, 4)).is_empty());

    // Empty swarm (no members, no names, no subagent status): only the stats line.
    let data = InfoWidgetData {
        swarm_info: Some(SwarmInfo::default()),
        ..Default::default()
    };
    let framed = super::render_swarm_widget(&data, Rect::new(0, 0, 40, 4)).settle();
    assert_eq!(framed.lines.len(), 1, "expected only the stats line");
    // session_count == 0 and client_count == None: stats line is just the bee icon.
    let text = lines_text(&framed.all_lines());
    assert!(
        !text.contains("0s"),
        "zero sessions must not render: {text}"
    );

    // Zero-size rect must not panic or underflow.
    let _ = super::render_swarm_widget(&data, Rect::new(0, 0, 0, 0));
    let mut member_data = data.clone();
    member_data.swarm_info.as_mut().unwrap().members = vec![SwarmMemberStatus {
        session_id: "abc".to_string(),
        friendly_name: None,
        status: "running".to_string(),
        detail: None,
        task_label: None,
        role: None,
        is_headless: None,
        live_attachments: None,
        status_age_secs: None,
        output_tail: None,
        report_back_to_session_id: None,
        todo_progress: None,
        todo_items: Vec::new(),
        runtime: crate::protocol::SwarmMemberRuntime::default(),
    }];
    let _ = super::render_swarm_widget(&member_data, Rect::new(0, 0, 0, 0));
    let _ = super::render_swarm_widget(&member_data, Rect::new(0, 0, 3, 1));
}

#[test]
fn swarm_widget_caps_member_rows_for_large_swarms() {
    let members: Vec<SwarmMemberStatus> = (0..500)
        .map(|i| SwarmMemberStatus {
            session_id: format!("session-{i:04}"),
            friendly_name: Some(format!("worker-{i}")),
            status: "running".to_string(),
            detail: Some("very long detail text that should be truncated".repeat(4)),
            task_label: None,
            role: None,
            is_headless: None,
            live_attachments: None,
            status_age_secs: None,
            output_tail: None,
            report_back_to_session_id: None,
            todo_progress: None,
            todo_items: Vec::new(),
            runtime: crate::protocol::SwarmMemberRuntime::default(),
        })
        .collect();
    let data = InfoWidgetData {
        swarm_info: Some(SwarmInfo {
            session_count: 500,
            client_count: Some(3),
            members,
            session_names: (0..500).map(|i| format!("worker-{i}")).collect(),
            ..Default::default()
        }),
        ..Default::default()
    };

    let framed = super::render_swarm_widget(&data, Rect::new(0, 0, 30, 10));
    // Stats on the border, at most 3 member rows regardless of swarm size.
    assert_eq!(framed.lines.len(), 3, "expected capped member rows");
    let text = lines_text(&framed.all_lines());
    assert!(text.contains("500s"), "got: {text}");
    assert!(text.contains("3 clients"), "got: {text}");
}

#[test]
fn background_widget_handles_empty_and_large_task_lists() {
    // No background info: renders nothing.
    let data = InfoWidgetData::default();
    assert!(super::render_background_widget(&data, Rect::new(0, 0, 40, 4)).is_empty());

    // running_count == 0: summary is suppressed even if stale task names linger.
    let info = BackgroundInfo {
        running_count: 0,
        running_tasks: vec!["stale".to_string()],
        ..Default::default()
    };
    assert!(super::render_background_compact(&info, 40).is_empty());

    // Large task list: summary + 3 rows + overflow line, no panic at tiny width.
    let info = BackgroundInfo {
        running_count: 200,
        running_tasks: (0..200).map(|i| format!("task-{i}")).collect(),
        progress_detail: Some("42% · working".to_string()),
        ..Default::default()
    };
    let data = InfoWidgetData {
        background_info: Some(info.clone()),
        ..Default::default()
    };
    let framed = super::render_background_widget(&data, Rect::new(0, 0, 40, 8));
    assert_eq!(
        framed.lines.len(),
        6,
        "6 task rows; summary and overflow on the border"
    );
    let text = lines_text(&framed.all_lines());
    assert!(text.contains("200 running"), "got: {text}");
    assert!(text.contains("+194 more"), "got: {text}");
    // Zero-size rect must not panic (row width clamps to a minimum).
    let _ = super::render_background_widget(&data, Rect::new(0, 0, 0, 0));
}

#[test]
fn background_widget_renders_session_task_rows_with_progress() {
    use crate::tui::{BackgroundTaskRow, BackgroundTaskRowStatus};
    let row = |id: &str, label: &str, percent: Option<f32>, status| BackgroundTaskRow {
        task_id: id.to_string(),
        label: label.to_string(),
        percent,
        status,
        completed_at: None,
    };
    let info = BackgroundInfo {
        rows: vec![
            row(
                "a",
                "integration tests",
                Some(30.0),
                BackgroundTaskRowStatus::Failed,
            ),
            row(
                "b",
                "release build",
                Some(100.0),
                BackgroundTaskRowStatus::Completed,
            ),
            row(
                "c",
                "cargo test",
                Some(50.0),
                BackgroundTaskRowStatus::Running,
            ),
        ],
        ..Default::default()
    };
    let data = InfoWidgetData {
        background_info: Some(info.clone()),
        ..Default::default()
    };
    assert!(data.has_data_for(WidgetKind::BackgroundTasks));

    let framed = super::render_background_widget(&data, Rect::new(0, 0, 40, 8));
    let rows: Vec<String> = framed
        .lines
        .iter()
        .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect();
    assert_eq!(
        rows,
        vec![
            "◌ cargo test  ━━━╺── 50%",
            "✓ release build  ━━━━━━ 100%",
            "× integration tests  ━━──── failed",
        ]
    );
    let title = lines_text(framed.title.as_slice());
    assert!(title.contains("Background · 1 running"), "got: {title}");

    // Height-limited: newest rows first, the rest collapse into the footer.
    let framed = super::render_background_widget(&data, Rect::new(0, 0, 40, 1));
    assert_eq!(framed.lines.len(), 1);
    assert!(lines_text(&framed.all_lines()).contains("+2 more"));

    // Only finished tasks: still renders, without a running count.
    let finished = InfoWidgetData {
        background_info: Some(BackgroundInfo {
            rows: vec![row(
                "b",
                "release build",
                None,
                BackgroundTaskRowStatus::Completed,
            )],
            ..Default::default()
        }),
        ..Default::default()
    };
    let framed = super::render_background_widget(&finished, Rect::new(0, 0, 40, 4));
    assert_eq!(lines_text(framed.title.as_slice()), "⏳ \nBackground");

    // Compact (overview) form mirrors its computed height.
    let compact = super::render_background_compact(&info, 40);
    assert_eq!(compact.len(), 4);
}

#[test]
fn running_background_task_outranks_passive_stat_widgets() {
    use crate::tui::{BackgroundTaskRow, BackgroundTaskRowStatus};
    let row = |status| BackgroundTaskRow {
        task_id: "t".to_string(),
        label: "cargo test".to_string(),
        percent: Some(10.0),
        status,
        completed_at: None,
    };
    let with = |status| InfoWidgetData {
        background_info: Some(BackgroundInfo {
            rows: vec![row(status)],
            ..Default::default()
        }),
        ..Default::default()
    };
    for status in [
        BackgroundTaskRowStatus::Running,
        BackgroundTaskRowStatus::Failed,
    ] {
        let data = with(status);
        let bg = data.effective_priority(WidgetKind::BackgroundTasks);
        assert!(bg < data.effective_priority(WidgetKind::KvCache));
        assert!(bg < data.effective_priority(WidgetKind::ModelInfo));
        assert!(bg < data.effective_priority(WidgetKind::UsageLimits));
    }
    let done = with(BackgroundTaskRowStatus::Completed);
    assert_eq!(
        done.effective_priority(WidgetKind::BackgroundTasks),
        WidgetKind::BackgroundTasks.priority()
    );
}

#[test]
fn background_widget_and_compact_share_summary_format() {
    let info = BackgroundInfo {
        running_count: 4,
        running_tasks: vec![
            "selfdev build".to_string(),
            "train.py".to_string(),
            "cargo test".to_string(),
            "download".to_string(),
        ],
        progress_summary: Some("selfdev build".to_string()),
        progress_detail: Some("[#####-------] 42% · Building (parsed)".to_string()),
        memory_agent_active: false,
        memory_agent_turns: 0,
        rows: Vec::new(),
    };
    let data = InfoWidgetData {
        background_info: Some(info.clone()),
        ..Default::default()
    };

    let framed = super::render_background_widget(&data, Rect::new(0, 0, 40, 1));
    let widget_text = lines_text(&framed.all_lines());
    let compact = super::render_background_compact(&info, 40);

    // The framed widget carries the summary on its top border and the compact
    // (Overview) form carries it inline on its first row: same summary text.
    let title = lines_text(framed.title.as_slice());
    let compact_head = lines_text(&compact[..1]);
    assert_eq!(title, compact_head);
    assert!(widget_text.contains("+3 more"), "got: {widget_text}");
    assert!(lines_text(&compact).contains("+1 more"));
    assert!(widget_text.contains("Background"), "got: {widget_text}");
    assert!(widget_text.contains("4"), "got: {widget_text}");
    assert!(!widget_text.contains("mem:"), "got: {widget_text}");
    assert!(widget_text.contains("selfdev build"), "got: {widget_text}");
    assert!(widget_text.contains("[#####-------]"), "got: {widget_text}");
    let compact_text = lines_text(&compact);
    assert!(compact_text.contains("train.py"), "got: {compact_text}");
    assert!(compact_text.contains("cargo test"), "got: {compact_text}");
}

#[test]
fn sticky_placement_clamps_width_to_current_margin() {
    {
        let mut guard = super::get_or_init_state();
        if let Some(state) = guard.as_mut() {
            state.enabled = true;
            state.placements.clear();
            state.anchors.clear();
            state.widget_states.clear();
        }
    }

    let data = InfoWidgetData {
        session_name: Some("sauropod".to_string()),
        queue_mode: Some(true),
        ..Default::default()
    };
    let area = Rect::new(0, 0, 100, 10);

    // First frame places a wide widget.
    let first = calculate_placements(
        area,
        &Margins {
            right_widths: vec![30; 10],
            left_widths: Vec::new(),
            centered: false,
            ..Default::default()
        },
        &data,
    );
    assert!(!first.is_empty(), "expected initial placement");
    assert_eq!(first[0].rect.width, 30);

    // Second frame shrinks margin by 4 columns (within sticky tolerance).
    let second_margins = vec![26; 10];
    let second = calculate_placements(
        area,
        &Margins {
            right_widths: second_margins.clone(),
            left_widths: Vec::new(),
            centered: false,
            ..Default::default()
        },
        &data,
    );
    assert!(!second.is_empty(), "expected sticky placement");

    let p = &second[0];
    let row_start = p.rect.y.saturating_sub(area.y) as usize;
    let row_end = row_start + p.rect.height as usize;
    let min_margin = second_margins[row_start..row_end]
        .iter()
        .copied()
        .min()
        .unwrap_or(0);
    assert!(
        p.rect.width <= min_margin,
        "sticky width {} exceeded current margin {}",
        p.rect.width,
        min_margin
    );
}

#[test]
fn placements_never_include_border_only_widgets() {
    {
        let mut guard = super::get_or_init_state();
        if let Some(state) = guard.as_mut() {
            state.enabled = true;
            state.placements.clear();
            state.anchors.clear();
            state.widget_states.clear();
        }
    }

    let data = InfoWidgetData {
        model: Some("gpt-test".to_string()),
        session_count: Some(2),
        context_info: Some(crate::prompt::ContextInfo {
            system_prompt_chars: 24_000,
            total_chars: 40_000,
            ..Default::default()
        }),
        queue_mode: Some(true),
        memory_info: Some(MemoryInfo {
            total_count: 1,
            ..Default::default()
        }),
        swarm_info: Some(SwarmInfo {
            session_count: 2,
            ..Default::default()
        }),
        background_info: Some(BackgroundInfo {
            running_count: 1,
            running_tasks: vec!["bash".to_string()],
            ..Default::default()
        }),
        usage_info: Some(UsageInfo {
            provider: UsageProvider::Anthropic,
            primary_limit_label: Some("5-hour".to_string()),
            five_hour: 0.35,
            secondary_limit_label: Some("Weekly".to_string()),
            seven_day: 0.62,
            available: true,
            ..Default::default()
        }),
        ..Default::default()
    };

    let placements = calculate_placements(
        Rect::new(0, 0, 100, 10),
        &Margins {
            right_widths: vec![40; 10],
            left_widths: Vec::new(),
            centered: false,
            ..Default::default()
        },
        &data,
    );

    assert!(
        placements.iter().all(|p| p.rect.height > 2),
        "found border-only widget placement: {:?}",
        placements
    );
}

/// The compact overview page must render exactly as many lines as
/// `compute_page_layout` reserved for it. A mismatch either clips the last
/// sections (background tasks were the historical victim, since they render
/// last) or leaves blank reserved rows.
#[test]
fn compact_page_height_estimate_matches_rendered_lines() {
    use super::InfoPageKind;

    // No memory so the only candidate page is CompactOnly, and the
    // background section (rendered last) is included.
    let data = InfoWidgetData {
        model: Some("claude-test-1".to_string()),
        provider_name: Some("anthropic".to_string()),
        session_count: Some(2),
        context_info: Some(crate::prompt::ContextInfo {
            system_prompt_chars: 10_000,
            total_chars: 30_000,
            ..Default::default()
        }),
        background_info: Some(BackgroundInfo {
            running_count: 2,
            running_tasks: vec!["bash".to_string(), "task".to_string()],
            ..Default::default()
        }),
        usage_info: Some(UsageInfo {
            provider: UsageProvider::Anthropic,
            primary_limit_label: Some("5-hour".to_string()),
            five_hour: 0.3,
            secondary_limit_label: Some("Weekly".to_string()),
            seven_day: 0.5,
            available: true,
            ..Default::default()
        }),
        cache_hit_info: Some(CacheHitInfo {
            reported_input_tokens: 1_000,
            prompt_tokens: Some(1_000),
            read_tokens: 800,
            ..Default::default()
        }),
        ..Default::default()
    };

    let inner = Rect::new(0, 0, 38, 30);
    let layout = super::compute_page_layout(&data, inner.width as usize, inner.height);
    assert_eq!(layout.pages.len(), 1, "expected a single compact page");
    assert_eq!(layout.pages[0].kind, InfoPageKind::CompactOnly);

    let lines = super::render_page(InfoPageKind::CompactOnly, &data, inner);
    assert_eq!(
        lines.len() as u16,
        layout.pages[0].height,
        "compact page height estimate must match rendered line count \
         (background section is rendered last and gets clipped on mismatch)"
    );
}

/// Same consistency check for a cost-based (API key) provider, whose usage
/// section renders a single line.
#[test]
fn compact_page_height_matches_for_cost_based_usage() {
    use super::InfoPageKind;

    let data = InfoWidgetData {
        model: Some("gpt-test".to_string()),
        background_info: Some(BackgroundInfo {
            running_count: 1,
            running_tasks: vec!["bash".to_string()],
            ..Default::default()
        }),
        usage_info: Some(UsageInfo {
            provider: UsageProvider::CostBased,
            total_cost: 0.42,
            input_tokens: 10_000,
            output_tokens: 2_000,
            available: true,
            ..Default::default()
        }),
        ..Default::default()
    };

    let inner = Rect::new(0, 0, 38, 30);
    let layout = super::compute_page_layout(&data, inner.width as usize, inner.height);
    assert_eq!(layout.pages.len(), 1);
    assert_eq!(layout.pages[0].kind, InfoPageKind::CompactOnly);

    let lines = super::render_page(InfoPageKind::CompactOnly, &data, inner);
    assert_eq!(lines.len() as u16, layout.pages[0].height);
}

/// Visual gallery: draws every text widget through the real
/// `render_single_widget` path. Run with
/// `justrust test -p jcode-tui --lib widget_gallery -- --nocapture`.
#[test]
fn widget_gallery() {
    use super::{
        AmbientWidgetData, CompactionInfo, DirtyFile, GitInfo, Side, WidgetPlacement,
        render_single_widget,
    };
    let now = Instant::now();
    let agent_file = std::path::PathBuf::from("/repo/src/tui/info_widget.rs");
    let data = InfoWidgetData {
        memory_info: Some(MemoryInfo {
            total_count: 42,
            project_count: 30,
            global_count: 12,
            activity: Some(MemoryActivity {
                state: MemoryState::Idle,
                state_since: now - Duration::from_secs(40),
                pipeline: None,
                recent_events: vec![MemoryEvent {
                    kind: MemoryEventKind::MemoryInjected {
                        count: 2,
                        prompt_chars: 318,
                        age_ms: 44,
                        preview: "prefers terse answers".to_string(),
                        items: Vec::new(),
                    },
                    timestamp: now - Duration::from_secs(30),
                    detail: None,
                }],
            }),
            ..Default::default()
        }),
        swarm_info: Some(SwarmInfo {
            managed_members: {
                let m = |id: &str, status: &str, role, detail: Option<&str>, todo, age| {
                    let mut s = managed_member(id, status, role);
                    s.detail = detail.map(str::to_string);
                    s.output_tail = None;
                    s.todo_progress = todo;
                    s.status_age_secs = Some(age);
                    s
                };
                vec![
                    m(
                        "lead",
                        "running",
                        Some("coordinator"),
                        Some("waiting on reviewer"),
                        Some((1, 3)),
                        4,
                    ),
                    m(
                        "builder",
                        "running",
                        None,
                        Some("wiring commits widget"),
                        Some((2, 5)),
                        2,
                    ),
                    m(
                        "reviewer",
                        "failed",
                        None,
                        Some("cargo test failed: 3 tests"),
                        None,
                        40,
                    ),
                    m(
                        "research",
                        "completed",
                        None,
                        Some("mapped the swarm code"),
                        None,
                        300,
                    ),
                    m("docs", "ready", None, None, None, 90),
                    m("lint", "completed", None, Some("clippy clean"), None, 600),
                    m(
                        "bench",
                        "completed",
                        None,
                        Some("no regressions"),
                        None,
                        900,
                    ),
                ]
            },
            ..Default::default()
        }),
        background_info: Some(BackgroundInfo {
            running_count: 5,
            running_tasks: vec![
                "selfdev build".into(),
                "cargo test -p jcode-tui".into(),
                "train.py".into(),
                "download weights".into(),
                "lint".into(),
            ],
            progress_summary: Some("selfdev build".into()),
            progress_detail: Some("[#####-------] 42% · Building".into()),
            ..Default::default()
        }),
        compaction_info: Some(CompactionInfo {
            is_compacting: false,
            compacted_messages: 184,
            active_messages: 36,
            summary_chars: 9_800,
            mode: "semantic".into(),
        }),
        ambient_info: Some(AmbientWidgetData {
            show_widget: true,
            status: crate::ambient::AmbientStatus::Running {
                detail: "triaging issues".into(),
            },
            queue_count: 3,
            next_queue_preview: Some("check CI on master".into()),
            last_run_ago: Some("12m ago".into()),
            next_wake: Some("in 18m".into()),
            budget_percent: Some(0.62),
            reminder_count: 0,
            next_reminder_preview: None,
            last_summary: None,
            next_reminder_wake: None,
        }),
        usage_info: Some(UsageInfo {
            provider: UsageProvider::Anthropic,
            primary_limit_label: Some("5h".into()),
            secondary_limit_label: Some("7d".into()),
            five_hour: 0.34,
            five_hour_resets_at: Some("2h 10m".into()),
            seven_day: 0.61,
            seven_day_resets_at: Some("3d".into()),
            available: true,
            ..Default::default()
        }),
        cache_hit_info: Some(CacheHitInfo {
            reported_input_tokens: 20_000,
            prompt_tokens: Some(38_000),
            last_prompt_tokens: Some(10_000),
            read_tokens: 15_000,
            creation_tokens: 3_000,
            optimal_input_tokens: 16_667,
            last_reported_input_tokens: Some(10_000),
            last_read_tokens: Some(9_400),
            last_creation_tokens: Some(0),
            last_optimal_input_tokens: Some(9_895),
            miss_attributions: vec![
                CacheMissAttribution {
                    turn_number: 20,
                    call_index: 1,
                    missed_tokens: 69_000,
                    reason: "provider switch".into(),
                },
                CacheMissAttribution {
                    turn_number: 27,
                    call_index: 2,
                    missed_tokens: 12_000,
                    reason: "system prompt changed".into(),
                },
            ],
        }),
        provider_name: Some("openai".into()),
        service_tier: Some("priority".into()),
        connection_type: Some("websocket".into()),
        tokens_per_second: Some(61.7),
        upstream_provider: Some("fireworks".into()),
        session_name: Some("sunflower".into()),
        session_count: Some(3),
        git_info: Some(GitInfo {
            branch: "master".into(),
            modified: 9,
            dirty_files: [
                ('M', "src/tui/info_widget.rs", 240, 180),
                ('A', "src/tui/info_widget_frame.rs", 274, 0),
                ('M', "src/tui/info_widget_todos.rs", 40, 23),
                ('M', "src/tui/info_widget_git.rs", 120, 46),
                ('D', "src/tui/old_widget.rs", 0, 88),
                ('M', "src/tui/info_widget_usage.rs", 30, 16),
                ('M', "src/tui/info_widget_model.rs", 20, 12),
            ]
            .into_iter()
            .map(|(status, path, a, r)| DirtyFile {
                status,
                path: path.into(),
                added: Some(a),
                removed: Some(r),
                ..Default::default()
            })
            .collect(),
            dirty_total: 9,
            added_total: 760,
            removed_total: 380,
            repo_root: Some("/repo".into()),
            ahead: 2,
            recent_commits: {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs() as i64;
                [
                    (
                        "e4b4d3a",
                        "tui: swarm dock lists each agent",
                        120,
                        true,
                        363,
                        35,
                    ),
                    (
                        "fdad464",
                        "tui: add info widget gallery test",
                        480,
                        true,
                        209,
                        0,
                    ),
                    (
                        "e205d0f",
                        "tui: widgets put headers on borders",
                        900,
                        false,
                        906,
                        503,
                    ),
                    (
                        "4f6bf8e",
                        "replay reasoning before its text",
                        4200,
                        false,
                        40,
                        12,
                    ),
                    ("a526dd5", "tui: drop dot separators", 9000, false, 6, 9),
                    ("e34070a", "Revert diagram pane cycle", 12000, false, 30, 44),
                ]
                .into_iter()
                .map(|(hash, subject, ago, unpushed, a, r)| super::RecentCommit {
                    hash: hash.into(),
                    subject: subject.into(),
                    timestamp: now - ago,
                    unpushed,
                    added: Some(a),
                    removed: Some(r),
                })
                .collect()
            },
            ..Default::default()
        }),
        agent_edited: std::sync::Arc::new([agent_file].into_iter().collect()),
        ..Default::default()
    };

    let kinds = [
        WidgetKind::MemoryActivity,
        WidgetKind::SwarmStatus,
        WidgetKind::BackgroundTasks,
        WidgetKind::Compaction,
        WidgetKind::AmbientMode,
        WidgetKind::UsageLimits,
        WidgetKind::KvCache,
        WidgetKind::ModelInfo,
        WidgetKind::Tips,
        WidgetKind::GitStatus,
        WidgetKind::Commits,
        WidgetKind::Overview,
    ];
    for width in [12u16, 24, 44] {
        for kind in kinds {
            let cap = if kind == WidgetKind::Overview { 40 } else { 12 };
            let mut h = calculate_widget_height(kind, &data, width, cap);
            if h <= 2 {
                // Ambient/Tips are gated off in layout; draw them anyway.
                h = 8;
            }
            let backend = ratatui::backend::TestBackend::new(width, h);
            let mut terminal = ratatui::Terminal::new(backend).unwrap();
            terminal
                .draw(|f| {
                    let placement = WidgetPlacement {
                        kind,
                        rect: f.area(),
                        side: Side::Left,
                    };
                    render_single_widget(f, &placement, &data);
                })
                .unwrap();
            let buf = terminal.backend().buffer().clone();
            let bottom = (0..h)
                .find(|&y| buf[(0, y)].symbol() == "╰")
                .unwrap_or_else(|| panic!("{kind:?} at width {width} has no bottom border"));
            let footer: String = (1..width - 1).map(|x| buf[(x, bottom)].symbol()).collect();
            assert!(
                footer.chars().any(|ch| ch != '─' && !ch.is_whitespace()),
                "{kind:?} at width {width} has an empty footer: {footer}"
            );
            for redundant in [
                "Runtime details",
                "Session overview",
                "Working tree",
                "Recent history",
                "Provider limits",
                "Scheduled activity",
                "Background work",
                "Agent sessions",
                "Memory activity",
            ] {
                assert!(
                    !footer.contains(redundant),
                    "{kind:?} has redundant footer: {footer}"
                );
            }
            println!("{kind:?}");
            for y in 0..h {
                let row: String = (0..width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect();
                println!("{}", row.trim_end());
            }
            println!();
        }
    }
}

#[test]
fn special_widgets_have_bottom_border_text() {
    use super::{CloudProbeState, CloudStatusInfo, Side, WidgetPlacement};
    use ratatui::{Terminal, backend::TestBackend};
    let data = InfoWidgetData {
        workspace_rows: vec![crate::tui::workspace_map::VisibleWorkspaceRow {
            workspace: 0,
            is_current: true,
            focused_index: Some(0),
            sessions: vec![crate::tui::workspace_map::WorkspaceSessionTile::new("fox")],
        }],
        cloud_info: Some(CloudStatusInfo {
            host: "test-host".into(),
            remote_dir: "/workspace".into(),
            local_dir: None,
            local_is_source: false,
            started_at: std::time::SystemTime::now(),
            last_success: None,
            probe_duration_ms: None,
            state: CloudProbeState::Loading,
            machine: None,
            last_transfer_at: None,
        }),
        ..Default::default()
    };
    // The diagram branch must retain its footer even while image data is absent.
    for kind in [
        WidgetKind::Diagrams,
        WidgetKind::WorkspaceMap,
        WidgetKind::CloudVm,
    ] {
        for width in [12u16, 24, 44] {
            let mut terminal = Terminal::new(TestBackend::new(width, 12)).unwrap();
            terminal
                .draw(|f| {
                    super::render_all(
                        f,
                        &[WidgetPlacement {
                            kind,
                            rect: f.area(),
                            side: Side::Right,
                        }],
                        &data,
                    );
                })
                .unwrap();
            let buf = terminal.backend().buffer();
            let bottom = (0..12)
                .find(|&y| buf[(0, y)].symbol() == "╰")
                .expect("rendered widget must have a bottom border");
            let footer: String = (1..width - 1).map(|x| buf[(x, bottom)].symbol()).collect();
            assert!(
                footer.chars().any(char::is_alphabetic),
                "{kind:?}: {footer}"
            );
            for redundant in ["Workspace layout", "Remote machine", "Latest diagram"] {
                assert!(!footer.contains(redundant), "{kind:?}: {footer}");
            }
        }
    }
}

#[test]
fn metadata_footers_add_distinct_compact_facts() {
    let mut data = InfoWidgetData {
        observed_context_tokens: Some(25_000),
        context_limit: Some(100_000),
        diagrams: vec![super::DiagramInfo {
            hash: 0,
            width: 640,
            height: 480,
            label: None,
        }],
        ..Default::default()
    };
    assert_eq!(super::overview_footer(&data), "75K ctx left");
    assert_eq!(super::diagram_footer(&data), "640×480");
    data.diagrams.push(data.diagrams[0].clone());
    assert_eq!(super::diagram_footer(&data), "640×480 · 1/2");
    data.context_info_stale = true;
    assert_eq!(super::overview_footer(&data), "context syncing");
    data.context_info_stale = false;
    data.observed_context_tokens = Some(110_000);
    assert_eq!(super::overview_footer(&data), "0 ctx left");

    data.compaction_info = Some(super::CompactionInfo {
        is_compacting: false,
        compacted_messages: 20,
        active_messages: 4,
        summary_chars: 4_000,
        mode: "semantic".into(),
    });
    let framed = super::render_compaction_widget(&data, Rect::new(0, 0, 40, 6));
    let body = lines_text_concat(&framed.lines);
    let footer = lines_text_concat(framed.footer.as_slice());
    assert!(body.contains("20 old · 4 active"));
    assert!(footer.contains("tok") && footer.contains("semantic"));
    assert!(!body.contains("tok") && !body.contains("semantic"));
    assert!(framed.title_right.is_none());
}

fn footer_probe(kind: WidgetKind, data: &InfoWidgetData, width: u16) -> (String, String) {
    use ratatui::{Terminal, backend::TestBackend};

    let mut terminal = Terminal::new(TestBackend::new(width, 16)).unwrap();
    terminal
        .draw(|frame| {
            super::render_all(
                frame,
                &[super::WidgetPlacement {
                    kind,
                    rect: frame.area(),
                    side: super::Side::Right,
                }],
                data,
            );
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    let bottom = (0..16)
        .find(|&y| buffer[(0, y)].symbol() == "╰")
        .expect("public renderer must preserve the bottom border");
    assert_eq!(buffer[(width - 1, bottom)].symbol(), "╯");
    let row = |y| {
        (0..width)
            .map(|x| buffer[(x, y)].symbol())
            .collect::<String>()
    };
    let footer = row(bottom);
    assert!(
        footer
            .chars()
            .any(|ch| !ch.is_whitespace() && !"╰╯─".contains(ch)),
        "{kind:?} at {width} cells lost its footer: {footer}"
    );
    let above = (0..bottom).map(row).collect::<Vec<_>>().join("\n");
    (above, footer)
}

#[test]
fn public_runtime_footer_keeps_throughput_once_without_wrapping() {
    let data = InfoWidgetData {
        provider_name: Some("openai".into()),
        service_tier: Some("priority".into()),
        connection_type: Some("websocket".into()),
        tokens_per_second: Some(61.7),
        ..Default::default()
    };
    for width in [8, 12, 24, 44] {
        let (above, footer) = footer_probe(WidgetKind::ModelInfo, &data, width);
        assert!(!above.contains("tok/s"));
        assert!(!footer.contains("Runtime"));
        if width >= 24 {
            assert!(footer.contains("62 tok/s"), "{footer}");
            assert_eq!(footer.matches("62 tok/s").count(), 1);
        }
        if width == 44 {
            assert!(above.contains("fast tier") && above.contains("websocket"));
        }
    }
}

#[test]
fn public_memory_footer_drops_stale_trace_and_keeps_scope() {
    let now = Instant::now();
    let mut data = InfoWidgetData {
        memory_info: Some(MemoryInfo {
            total_count: 7,
            project_count: 4,
            global_count: 3,
            activity: Some(MemoryActivity {
                state: MemoryState::Embedding,
                state_since: now,
                pipeline: None,
                recent_events: vec![MemoryEvent {
                    kind: MemoryEventKind::MemoryInjected {
                        count: 2,
                        prompt_chars: 318,
                        age_ms: 44,
                        preview: "prefers terse answers".into(),
                        items: Vec::new(),
                    },
                    timestamp: now,
                    detail: None,
                }],
            }),
            ..Default::default()
        }),
        ..Default::default()
    };
    for width in [8, 12, 24, 44] {
        let (above, footer) = footer_probe(WidgetKind::MemoryActivity, &data, width);
        assert!(!above.contains("Trace:"));
        if width >= 24 {
            assert!(footer.contains("Trace:"), "{footer}");
        }
    }
    let activity = data
        .memory_info
        .as_mut()
        .unwrap()
        .activity
        .as_mut()
        .unwrap();
    activity.state = MemoryState::Idle;
    activity.state_since = now - Duration::from_secs(60);
    for width in [8, 12, 24, 44] {
        let (above, footer) = footer_probe(WidgetKind::MemoryActivity, &data, width);
        assert!(!above.contains("Trace:") && !footer.contains("Trace:"));
        if width >= 24 {
            assert!(footer.contains("4 project · 3 global"), "{footer}");
            assert!(!above.contains("4 project"));
        }
    }
}

#[test]
fn public_swarm_footer_tracks_output_clients_and_overflow() {
    let mut first = managed_member("fox", "running", None);
    first.runtime.output_tokens = Some(12_000);
    let mut second = managed_member("owl", "completed", None);
    second.runtime.output_tokens = Some(8_000);
    let mut data = InfoWidgetData {
        swarm_info: Some(SwarmInfo {
            managed_members: vec![first, second],
            client_count: Some(1),
            ..Default::default()
        }),
        ..Default::default()
    };
    for width in [8, 12, 24, 44] {
        let (above, footer) = footer_probe(WidgetKind::SwarmStatus, &data, width);
        assert!(!above.contains("20K out"));
        if width >= 12 {
            assert!(footer.contains("20K out"), "{footer}");
        }
    }
    for member in &mut data.swarm_info.as_mut().unwrap().managed_members {
        member.runtime.output_tokens = None;
    }
    let (_, footer) = footer_probe(WidgetKind::SwarmStatus, &data, 44);
    assert!(footer.contains("1 client"), "{footer}");
    assert!(!footer.contains("1 clients"), "{footer}");

    data.swarm_info = Some(SwarmInfo {
        session_count: 4,
        session_names: vec!["one".into(), "two".into(), "three".into(), "four".into()],
        client_count: Some(1),
        ..Default::default()
    });
    for width in [8, 12, 24, 44] {
        let (above, footer) = footer_probe(WidgetKind::SwarmStatus, &data, width);
        assert!(!above.contains("1 client"));
        if width >= 12 {
            assert!(footer.contains("+1 more"), "{footer}");
        }
        if width == 44 {
            assert!(footer.contains("1 client"), "{footer}");
        }
    }
}

#[test]
fn public_background_footer_distinguishes_states_and_preserves_overflow() {
    use crate::tui::{BackgroundTaskRow, BackgroundTaskRowStatus};

    let row = |status| BackgroundTaskRow {
        task_id: "task".into(),
        label: "build".into(),
        percent: Some(100.0),
        status,
        completed_at: Some(Instant::now()),
    };
    let mut data = InfoWidgetData::default();
    for (status, legend) in [
        (BackgroundTaskRowStatus::Running, "◌ running"),
        (BackgroundTaskRowStatus::Completed, "✓ complete"),
        (BackgroundTaskRowStatus::Failed, "✗ failed"),
    ] {
        data.background_info = Some(super::BackgroundInfo {
            rows: vec![row(status)],
            ..Default::default()
        });
        for width in [8, 12, 24, 44] {
            let (above, footer) = footer_probe(WidgetKind::BackgroundTasks, &data, width);
            assert!(!above.contains(legend));
            if width >= 24 {
                assert!(footer.contains(legend), "{footer}");
            }
        }
    }
    data.background_info = Some(super::BackgroundInfo {
        rows: vec![
            row(BackgroundTaskRowStatus::Running),
            row(BackgroundTaskRowStatus::Completed),
        ],
        ..Default::default()
    });
    let (_, footer) = footer_probe(WidgetKind::BackgroundTasks, &data, 44);
    assert!(footer.contains("◌ running · ✓ complete"), "{footer}");

    data.background_info = Some(super::BackgroundInfo {
        running_count: 8,
        running_tasks: (0..8).map(|index| format!("task {index}")).collect(),
        ..Default::default()
    });
    let (_, footer) = footer_probe(WidgetKind::BackgroundTasks, &data, 44);
    assert!(footer.contains("+2 more"), "{footer}");
    data.background_info.as_mut().unwrap().running_tasks.clear();
    let (_, footer) = footer_probe(WidgetKind::BackgroundTasks, &data, 44);
    assert!(footer.contains("this session"), "{footer}");
}

#[test]
fn public_workspace_footer_counts_visible_sessions_and_running_state() {
    use crate::tui::workspace_map::{
        VisibleWorkspaceRow, WorkspaceSessionTile, WorkspaceSessionVisualState,
    };

    let mut data = InfoWidgetData {
        workspace_rows: vec![VisibleWorkspaceRow {
            workspace: 0,
            is_current: true,
            focused_index: Some(0),
            sessions: vec![WorkspaceSessionTile::new("雲")],
        }],
        ..Default::default()
    };
    let (above, footer) = footer_probe(WidgetKind::WorkspaceMap, &data, 44);
    assert!(above.contains("Workspace"));
    assert!(footer.contains("1 session"));
    assert!(!footer.contains("sessions"));

    data.workspace_rows[0].sessions[0].state = WorkspaceSessionVisualState::Running;
    data.workspace_rows.push(VisibleWorkspaceRow {
        workspace: 1,
        is_current: false,
        focused_index: Some(0),
        sessions: vec![
            WorkspaceSessionTile::with_state("狐", WorkspaceSessionVisualState::Running),
            WorkspaceSessionTile::new("bird"),
        ],
    });
    for width in [8, 12, 24, 44] {
        let (above, footer) = footer_probe(WidgetKind::WorkspaceMap, &data, width);
        assert!(!above.contains("3 sessions"));
        if width == 44 {
            assert!(footer.contains("3 sessions · 2 running"), "{footer}");
        }
    }
}

#[test]
fn public_diagram_footer_keeps_dimensions_when_image_is_not_cached() {
    let mut data = InfoWidgetData::default();
    let (_, footer) = footer_probe(WidgetKind::Diagrams, &data, 44);
    assert!(footer.contains("image pending"));
    data.diagrams = vec![
        super::DiagramInfo {
            hash: u64::MAX,
            width: 640,
            height: 480,
            label: None,
        };
        2
    ];
    for width in [8, 12, 24, 44] {
        let (above, footer) = footer_probe(WidgetKind::Diagrams, &data, width);
        assert!(!above.contains("640×480") && !above.contains("1/2"));
        if width >= 24 {
            assert!(footer.contains("640×480 · 1/2"), "{footer}");
        }
    }
}

#[test]
fn ambient_footer_moves_next_run_out_of_body() {
    let data = InfoWidgetData {
        ambient_info: Some(super::AmbientWidgetData {
            show_widget: true,
            status: crate::ambient::AmbientStatus::Idle,
            queue_count: 1,
            next_queue_preview: Some("check CI".into()),
            reminder_count: 0,
            next_reminder_preview: None,
            last_run_ago: None,
            last_summary: None,
            next_wake: Some("in 3m".into()),
            next_reminder_wake: None,
            budget_percent: None,
        }),
        ..Default::default()
    };
    let framed =
        super::render_widget_content(WidgetKind::AmbientMode, &data, Rect::new(0, 0, 40, 6));
    assert_eq!(lines_text_concat(framed.footer.as_slice()), "next in 3m");
    assert!(!lines_text_concat(&framed.lines).contains("3m"));
    assert_eq!(framed.lines.len(), 1);
}

#[test]
fn narrow_overview_background_row_keeps_progress_visible() {
    use crate::tui::{BackgroundTaskRow, BackgroundTaskRowStatus};
    use ratatui::{Terminal, backend::TestBackend};
    let data = InfoWidgetData {
        model: Some("claude-opus-5-5".into()),
        background_info: Some(BackgroundInfo {
            rows: vec![BackgroundTaskRow {
                task_id: "t".into(),
                label: "Live verification task".into(),
                percent: Some(20.0),
                status: BackgroundTaskRowStatus::Running,
                completed_at: None,
            }],
            ..Default::default()
        }),
        usage_info: Some(UsageInfo {
            provider: UsageProvider::Anthropic,
            five_hour: 0.3,
            seven_day: 0.5,
            available: true,
            ..Default::default()
        }),
        ..Default::default()
    };
    let (w, h) = (24u16, 9u16);
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| {
        super::render_all(
            f,
            &[super::WidgetPlacement {
                kind: WidgetKind::Overview,
                rect: Rect::new(0, 0, w, h),
                side: super::Side::Right,
            }],
            &data,
        )
    })
    .unwrap();
    let buf = term.backend().buffer().clone();
    let mut out = String::new();
    for y in 0..h {
        for x in 0..w {
            out.push_str(buf[(x, y)].symbol());
        }
        out.push('\n');
    }
    assert!(out.contains("◌ Live"), "{out}");
    assert!(
        out.contains("20%"),
        "progress must fit at overview width:\n{out}"
    );
}

#[test]
fn overview_never_hides_memory_data() {
    // The overview strips `memory_info` (the dedicated MemoryActivity widget
    // owns that content, matching master). Regression: MemoryActivity must NOT
    // be overview-mergeable, or placing the overview would suppress the only
    // renderer of memory data.
    let _lock = crate::tui::ui::render_state_test_lock();
    let area = Rect::new(0, 0, 38, 24);
    let margins = super::Margins {
        right_widths: vec![38; 24],
        ..Default::default()
    };
    let data = super::InfoWidgetData {
        queue_mode: Some(true),
        memory_info: Some(MemoryInfo {
            total_count: 4,
            ..Default::default()
        }),
        // A second section so the overview has content beyond memory and places.
        usage_info: Some(super::UsageInfo {
            provider: super::UsageProvider::CostBased,
            available: true,
            ..Default::default()
        }),
        ..Default::default()
    };
    assert!(!super::is_overview_mergeable(super::WidgetKind::Overview));
    assert!(!super::is_overview_mergeable(
        super::WidgetKind::MemoryActivity
    ));

    let outcome = super::super::info_widget_layout::calculate_placements_anchored(
        area,
        &margins,
        &data,
        true,
        &[],
    );
    assert!(
        outcome
            .visible
            .iter()
            .any(|p| p.kind == super::WidgetKind::Overview),
        "overview should place (queue_mode + usage sections): {:?}",
        outcome.visible.iter().map(|p| p.kind).collect::<Vec<_>>()
    );
    assert!(
        outcome
            .visible
            .iter()
            .any(|p| p.kind == super::WidgetKind::MemoryActivity),
        "memory activity must still place alongside the overview"
    );

    let backend = ratatui::backend::TestBackend::new(38, 24);
    let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
    terminal
        .draw(|frame| {
            super::render_all(frame, &outcome.visible, &data);
        })
        .expect("draw should not panic");
    let buf = terminal.backend().buffer();
    let rendered = (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered.contains('🧠'),
        "memory row must be visible somewhere while the overview is placed, got:\n{rendered}"
    );
}

#[test]
fn mergeable_anchor_yields_when_overview_arrives() {
    // `calculate_placements` mutates process-wide widget state (anchors,
    // placements, swarm stand-down). Hold the shared render-state lock for the
    // whole test so concurrent render tests (e.g.
    // `swarm_strip_stands_down_through_dock_blinks`) cannot interleave their
    // own placement calls between these frames.
    let _lock = crate::tui::ui::render_state_test_lock();
    use super::{GitInfo, RecentCommit, WidgetKind, calculate_placements};
    crate::tui::info_widget::clear_widget_placements_for_tests();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let data = super::InfoWidgetData {
        git_info: Some(GitInfo {
            branch: "fix/info_fields".into(),
            ahead: 2,
            recent_commits: [
                (
                    "e4b4d3a",
                    "tui: overview folds swarm and commits",
                    120,
                    true,
                ),
                ("fdad464", "tui: compact swarm rows", 300, true),
            ]
            .into_iter()
            .map(|(hash, subject, ago, unpushed)| RecentCommit {
                hash: hash.into(),
                subject: subject.into(),
                timestamp: now - ago,
                unpushed,
                added: None,
                removed: None,
            })
            .collect(),
            ..Default::default()
        }),
        ..Default::default()
    };
    let area = ratatui::layout::Rect::new(0, 0, 120, 30);
    let kinds = |p: &[super::WidgetPlacement]| p.iter().map(|p| p.kind).collect::<Vec<_>>();

    // Frame A: only a shallow pocket exists. Overview (min 8 rows) cannot
    // fit, but a lone Commits dock (min 1 + chrome) can, so Commits anchors.
    let shallow = super::Margins {
        right_widths: vec![44, 44, 44, 44, 44, 44, 44, 0, 0, 0, 0, 0, 0],
        scroll_top: 10,
        ..Default::default()
    };
    let mut pa = Vec::new();
    for _ in 0..6 {
        pa = calculate_placements(area, &shallow, &data);
    }
    assert_eq!(
        kinds(&pa),
        vec![WidgetKind::Commits],
        "frame A: only the shallow commits dock fits"
    );

    // Frame B: a deep pocket opens (terminal scrolled / wide lines gone) and
    // usage data arrives. The Overview must take over the deep pocket and
    // the mergeable Commits dock must fold in.
    let mut data2 = data.clone();
    data2.usage_info = Some(super::UsageInfo {
        provider: super::UsageProvider::CostBased,
        available: true,
        ..Default::default()
    });
    let deep = super::Margins {
        right_widths: vec![44; 30],
        scroll_top: 10,
        ..Default::default()
    };
    let pb = calculate_placements(area, &deep, &data2);
    assert_eq!(
        kinds(&pb),
        vec![WidgetKind::Overview],
        "frame B: overview must reclaim, commits fold in, got: {:?}",
        kinds(&pb)
    );
}

/// Regression (PR review): an anchored Overview must not silently swallow new
/// sections it can no longer fit.
///
/// Frame A anchors a model-only Overview in a small pocket. Frame B adds
/// commits and swarm members (sections that grow `compact_overview_height`
/// beyond the anchor's height while the terminal geometry stays the same).
/// The anchor must not hold its stale three-row rectangle: `render_overview_framed`
/// would draw nothing (the compact page no longer fits the inner height), the
/// mergeable standalone widgets stay suppressed, and `overview_carries_swarm`
/// also stands the inline swarm strip down - the user loses the panel, recent
/// commits, and managed-agent status, persisting on subsequent frames.
///
/// Expected behavior: either the anchor reflows to the new required height, or
/// it yields so the standalone widgets and the inline strip can carry the
/// content.
#[test]
fn anchored_overview_outgrown_by_new_activity_yields_or_reflows() {
    let _lock = crate::tui::ui::render_state_test_lock();
    use super::{
        GitInfo, RecentCommit, WidgetKind, calculate_placements, calculate_widget_height,
        swarm_strip_stands_down_for_dock,
    };
    crate::tui::info_widget::clear_widget_placements_for_tests();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let commit = |hash: &str, subject: &str, ago: i64| RecentCommit {
        hash: hash.into(),
        subject: subject.into(),
        timestamp: now - ago,
        unpushed: true,
        added: None,
        removed: None,
    };
    let member = |id: &str| crate::protocol::SwarmMemberStatus {
        session_id: id.to_string(),
        friendly_name: Some(id.to_string()),
        status: "running".to_string(),
        detail: None,
        task_label: None,
        role: None,
        is_headless: Some(true),
        live_attachments: None,
        status_age_secs: Some(2),
        output_tail: None,
        report_back_to_session_id: None,
        todo_progress: None,
        todo_items: Vec::new(),
        runtime: crate::protocol::SwarmMemberRuntime::default(),
    };

    // Frame A: model-only Overview (one runtime row + borders = 3 rows).
    let data = super::InfoWidgetData {
        model: Some("gpt-test".to_string()),
        ..Default::default()
    };
    let area = ratatui::layout::Rect::new(0, 0, 120, 30);
    let margins = super::Margins {
        right_widths: vec![44; 30],
        scroll_top: 10,
        ..Default::default()
    };
    let mut pa = Vec::new();
    for _ in 0..3 {
        pa = calculate_placements(area, &margins, &data);
    }
    let overview_a = pa
        .iter()
        .find(|p| p.kind == WidgetKind::Overview)
        .expect("frame A: model-only overview should anchor in the wide margin");
    assert_eq!(
        overview_a.rect.height, 3,
        "frame A: model-only overview is a 3-row panel"
    );

    // Frame B: commits and managed swarm members arrive. The same geometry
    // keeps a 30-row pocket, but the compact overview now needs more rows.
    let mut data2 = data.clone();
    data2.git_info = Some(GitInfo {
        branch: "fix/info_fields".into(),
        ahead: 1,
        recent_commits: vec![commit(
            "e4b4d3a",
            "tui: overview folds swarm and commits",
            120,
        )],
        ..Default::default()
    });
    data2.swarm_info = Some(super::SwarmInfo {
        managed_members: vec![member("w1"), member("w2")],
        ..Default::default()
    });
    let required = calculate_widget_height(WidgetKind::Overview, &data2, overview_a.rect.width, 30);
    assert!(
        required > overview_a.rect.height,
        "frame B: new sections must grow the required overview height"
    );

    let pb = calculate_placements(area, &margins, &data2);
    if let Some(overview_b) = pb.iter().find(|p| p.kind == WidgetKind::Overview) {
        // Path 1: the anchor reflowed to a height that can actually show the
        // (at least compact) overview content.
        let inner = overview_b.rect.height.saturating_sub(2);
        let mut overview_data = data2.clone();
        overview_data.memory_info = None;
        let compact = super::super::info_widget_overview::compute_page_layout(
            &overview_data,
            overview_b.rect.width.saturating_sub(2) as usize,
            inner,
        );
        assert!(
            compact.max_page_height > 0 && compact.max_page_height <= inner,
            "kept overview must fit a page: rect {:?} max_page_height {} inner {}",
            overview_b.rect,
            compact.max_page_height,
            inner
        );
        assert!(
            swarm_strip_stands_down_for_dock(),
            "a renderable overview carrying swarm rows keeps the strip down"
        );
    } else {
        // Path 2: no renderable Overview could be placed, so the mergeable
        // standalone widgets must come back and the swarm strip must be free
        // to render the managed agents (no phantom `overview_carries_swarm`).
        assert!(
            pb.iter().any(|p| p.kind == WidgetKind::Commits),
            "frame B: without a placeable overview the commits dock must return, got {:?}",
            pb.iter().map(|p| p.kind).collect::<Vec<_>>()
        );
        assert!(
            pb.iter().any(|p| p.kind == WidgetKind::SwarmStatus),
            "frame B: without a placeable overview the swarm dock must return"
        );
        assert!(
            !swarm_strip_stands_down_for_dock(),
            "frame B: the strip must be free when no overview carries the swarm rows"
        );
    }
}

/// Regression (greptile P1, PR 1709): a placed Overview whose pocket fits
/// only the front sections of the compact page must not suppress the
/// standalone fallbacks for the tail sections it truncates away.
///
/// A 10-row margin pocket hosts the Overview; Runtime + Todos + Background +
/// Usage rows fill its whole body, so the compact page truncates before the
/// Changes/Commits/Swarm sections. Before the fix, `overview_carries_swarm`
/// still stood the inline strip down and Phase 2 still suppressed the
/// SwarmStatus and Commits docks, so a running worker and a fresh unpushed
/// commit were visible nowhere (another pocket was available for them).
///
/// Expected, phase 1: with a second pocket free, the Commits/SwarmStatus
/// fallbacks place into it and the (now real) SwarmStatus dock owns the strip.
/// Expected, phase 2: with no other pocket at all, no dock can place, so the
/// inline strip must come back up - the agents surface there.
#[test]
fn overview_truncated_tail_keeps_fallbacks_and_strip() {
    let _lock = crate::tui::ui::render_state_test_lock();
    use super::{
        GitInfo, RecentCommit, WidgetKind, calculate_placements, swarm_strip_stands_down_for_dock,
    };
    use crate::tui::info_widget_overview::section_visibility;
    crate::tui::info_widget::clear_widget_placements_for_tests();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let member = |id: &str| crate::protocol::SwarmMemberStatus {
        session_id: id.to_string(),
        friendly_name: Some(id.to_string()),
        status: "running".to_string(),
        detail: None,
        task_label: None,
        role: None,
        is_headless: Some(true),
        live_attachments: None,
        status_age_secs: Some(2),
        output_tail: None,
        report_back_to_session_id: None,
        todo_progress: None,
        todo_items: Vec::new(),
        runtime: crate::protocol::SwarmMemberRuntime::default(),
    };
    let commit = |hash: &str, subject: &str, ago: i64| RecentCommit {
        hash: hash.into(),
        subject: subject.into(),
        timestamp: now - ago,
        unpushed: true,
        added: None,
        removed: None,
    };

    // Front sections: runtime rows (model) + background (1 header + 1 task
    // row) + usage (label + primary) + kv cache (1) + compaction (1).
    // Together they leave 1 free row of the 10-row pocket's 8 body rows,
    // so only the tail (Commits/Swarm) is truncated away.
    let data = super::InfoWidgetData {
        model: Some("gpt-test".to_string()),
        background_info: Some(super::BackgroundInfo {
            running_count: 1,
            running_tasks: vec!["bash".to_string()],
            ..Default::default()
        }),
        usage_info: Some(super::UsageInfo {
            provider: super::UsageProvider::Anthropic,
            available: true,
            primary_limit_label: Some("5h".to_string()),
            ..Default::default()
        }),
        cache_hit_info: Some(super::CacheHitInfo::default()),
        compaction_info: Some(super::CompactionInfo {
            is_compacting: false,
            compacted_messages: 184,
            active_messages: 36,
            summary_chars: 9_800,
            mode: "semantic".into(),
        }),
        git_info: Some(GitInfo {
            branch: "fix/info_fields".into(),
            ahead: 1,
            recent_commits: vec![commit(
                "e4b4d3a",
                "tui: overview folds swarm and commits",
                120,
            )],
            ..Default::default()
        }),
        swarm_info: Some(super::SwarmInfo {
            managed_members: vec![member("w1"), member("w2")],
            ..Default::default()
        }),
        ..Default::default()
    };

    // Right margin: a 10-row pocket (rows 0..10) and a second free pocket
    // (rows 14..24) so the fallbacks have somewhere to place.
    let area = ratatui::layout::Rect::new(0, 0, 120, 30);
    let mut right_widths = vec![0u16; 30];
    for w in right_widths.iter_mut().take(10) {
        *w = 44;
    }
    for w in right_widths.iter_mut().skip(14).take(10) {
        *w = 44;
    }
    let margins = super::Margins {
        right_widths,
        scroll_top: 10,
        ..Default::default()
    };

    let mut placements = Vec::new();
    for _ in 0..3 {
        placements = calculate_placements(area, &margins, &data);
    }

    let overview = placements
        .iter()
        .find(|p| p.kind == WidgetKind::Overview)
        .expect("the overview should anchor in the 10-row pocket");
    let vis = section_visibility(&data, overview.rect);
    assert!(
        !vis.commits && !vis.swarm,
        "pocket must truncate the tail sections before the fix's scenario, got {:?} for {:?}",
        vis,
        overview.rect
    );

    // Phase 1 - fallbacks released: with a second pocket free, Commits and
    // SwarmStatus must place there even though the placed Overview truncates
    // its tail sections. Before the fix they were suppressed and the agents
    // were visible nowhere despite the free pocket.
    assert!(
        placements.iter().any(|p| p.kind == WidgetKind::Commits),
        "the truncated overview must not suppress the Commits dock, got {:?}",
        placements.iter().map(|p| p.kind).collect::<Vec<_>>()
    );
    assert!(
        placements.iter().any(|p| p.kind == WidgetKind::SwarmStatus),
        "the truncated overview must not suppress the SwarmStatus dock"
    );
    // The strip legitimately stands down now: the SwarmStatus dock (not a
    // phantom overview) owns the swarm rows. This is the fixed steady state.
    assert!(
        swarm_strip_stands_down_for_dock(),
        "the placed SwarmStatus dock should own the strip instead of the truncated overview"
    );

    // Phase 2 - no room anywhere: keep only the single 10-row pocket so no
    // dock can place, and clear the anchors so the truncated Overview cannot
    // linger as one either. The agents must surface somewhere: the inline
    // swarm strip must come back up.
    crate::tui::info_widget::clear_widget_placements_for_tests();
    let mut right_widths = vec![0u16; 30];
    for w in right_widths.iter_mut().take(10) {
        *w = 44;
    }
    let margins = super::Margins {
        right_widths,
        scroll_top: 10,
        ..Default::default()
    };
    let placements = calculate_placements(area, &margins, &data);
    assert!(
        placements.iter().any(|p| p.kind == WidgetKind::Overview),
        "single-pocket frame: the truncated overview keeps its pocket, got {:?}",
        placements.iter().map(|p| p.kind).collect::<Vec<_>>()
    );
    assert!(
        !placements.iter().any(|p| p.kind == WidgetKind::SwarmStatus)
            && !placements.iter().any(|p| p.kind == WidgetKind::Commits),
        "single-pocket frame: no room for the docks, they must not phantom-place"
    );
    assert!(
        !swarm_strip_stands_down_for_dock(),
        "no dock owns the swarm rows, so the strip must come back up"
    );
}
