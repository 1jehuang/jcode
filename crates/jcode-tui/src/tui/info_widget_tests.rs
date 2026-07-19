use super::{
    BackgroundInfo, CacheHitInfo, CacheMissAttribution, GraphEdge, GraphNode, InfoWidgetData,
    Margins, MemoryActivity, MemoryEvent, MemoryEventKind, MemoryInfo, MemoryState, PipelineState,
    StepStatus, SwarmInfo, UsageInfo, UsageProvider, WidgetKind, calculate_placements,
    calculate_widget_height, effective_prompt_tokens, occasional_status_tip,
    render_kv_cache_widget, render_memory_compact, render_memory_widget, render_model_widget,
    render_todos_compact, render_todos_expanded, render_todos_widget, render_usage_compact,
    render_usage_widget, swarm_plan_todos, truncate_smart,
};
use crate::protocol::SwarmMemberStatus;
use ratatui::layout::Rect;
use std::time::{Duration, Instant};

#[test]
fn effective_prompt_tokens_handles_split_and_subset_accounting() {
    // Anthropic-style split accounting: `input` is only the uncached remainder,
    // so cache_read pushed beyond input means the true prompt is the sum.
    assert_eq!(effective_prompt_tokens(2449, 19499, 684), 22632);
    // OpenAI-style subset accounting: cached tokens are inside `input`.
    assert_eq!(effective_prompt_tokens(10000, 6000, 0), 10000);
    // No cache telemetry at all behaves like a plain input count.
    assert_eq!(effective_prompt_tokens(5000, 0, 0), 5000);
}

#[test]
fn cache_hit_ratio_uses_effective_prompt_for_split_providers() {
    // Mirrors a real Anthropic log line where read >> input and the old code
    // clamped the ratio to 100%.
    let cache = CacheHitInfo {
        reported_input_tokens: 2449,
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
    let lines = render_kv_cache_widget(&data, Rect::new(0, 0, 40, 5));
    let text = lines_text(&lines);

    assert_eq!(lines.len(), 4);
    assert!(text.contains("KV cache:"));
    assert!(text.contains("yield "));
    assert!(text.contains("90%"));
    assert!(text.contains("last "));
    assert!(text.contains("94%"));
    assert!(text.contains("session "));
    assert!(text.contains("39%"));
    assert!(text.contains("miss attribution"));
    assert!(text.contains("69k missed total"));
    assert!(text.contains("20>"));
    assert!(text.contains("69k miss"));
    assert!(text.contains("provider switch"));
}

#[test]
fn todos_widgets_show_item_and_aggregate_confidence() {
    let data = InfoWidgetData {
        todos: vec![
            crate::todo::TodoItem {
                group: None,
                id: "todo-1".to_string(),
                content: "Validate confidence UI".to_string(),
                status: "in_progress".to_string(),
                priority: "high".to_string(),
                confidence: Some(80),
                completion_confidence: None,
                confidence_history: Vec::new(),
                blocked_by: Vec::new(),
                assigned_to: None,
            },
            crate::todo::TodoItem {
                group: None,
                id: "todo-2".to_string(),
                content: "Ship completed item".to_string(),
                status: "completed".to_string(),
                priority: "medium".to_string(),
                confidence: Some(70),
                completion_confidence: Some(95),
                confidence_history: Vec::new(),
                blocked_by: Vec::new(),
                assigned_to: None,
            },
        ],
        ..Default::default()
    };

    let normal_text = lines_text(&render_todos_widget(&data, Rect::new(0, 0, 80, 8)));
    assert!(normal_text.contains("86%"));
    assert!(normal_text.contains("80%"));
    assert!(normal_text.contains("95%"));

    let expanded_text = lines_text(&render_todos_expanded(&data, Rect::new(0, 0, 80, 8)));
    assert!(expanded_text.contains("86%"));
    assert!(expanded_text.contains("80%"));
    assert!(expanded_text.contains("95%"));

    let compact_text = lines_text(&render_todos_compact(&data, Rect::new(0, 0, 80, 2)));
    assert!(compact_text.contains("86%"));
}

#[test]
fn todos_widgets_render_group_headers_when_groups_present() {
    let mk = |group: Option<&str>, id: &str, status: &str| crate::todo::TodoItem {
        group: group.map(|g| g.to_string()),
        id: id.to_string(),
        content: format!("task {id}"),
        status: status.to_string(),
        priority: "medium".to_string(),
        confidence: Some(80),
        completion_confidence: None,
        confidence_history: Vec::new(),
        blocked_by: Vec::new(),
        assigned_to: None,
    };
    let data = InfoWidgetData {
        todos: vec![
            mk(Some("optimize rendering"), "a", "completed"),
            mk(Some("optimize rendering"), "b", "in_progress"),
            mk(Some("fix scrollback"), "c", "pending"),
            mk(None, "d", "pending"),
        ],
        ..Default::default()
    };

    let expanded = lines_text_concat(&render_todos_expanded(&data, Rect::new(0, 0, 80, 14)));
    // Group headers appear with per-group progress counters, first-seen order,
    // and the ungrouped bucket renders under "Other".
    assert!(expanded.contains("optimize rendering"), "{expanded}");
    assert!(expanded.contains("1/2"), "{expanded}");
    assert!(
        expanded.contains("1/2 · confidence 80%"),
        "group confidence missing: {expanded}"
    );
    assert!(expanded.contains("fix scrollback"), "{expanded}");
    assert!(expanded.contains("Other"), "{expanded}");
    let opt_idx = expanded.find("optimize rendering").unwrap();
    let fix_idx = expanded.find("fix scrollback").unwrap();
    let other_idx = expanded.find("Other").unwrap();
    assert!(opt_idx < fix_idx, "first-seen group order: {expanded}");
    assert!(fix_idx < other_idx, "ungrouped bucket last: {expanded}");
}

#[test]
fn task_group_headers_render_their_own_weighted_confidence() {
    let mk = |group: &str, id: &str, priority: &str, confidence: u8| crate::todo::TodoItem {
        group: Some(group.to_string()),
        id: id.to_string(),
        content: format!("task {id}"),
        status: "pending".to_string(),
        priority: priority.to_string(),
        confidence: Some(confidence),
        completion_confidence: None,
        confidence_history: Vec::new(),
        blocked_by: Vec::new(),
        assigned_to: None,
    };
    let data = InfoWidgetData {
        todos: vec![
            mk("high confidence", "a", "high", 100),
            mk("high confidence", "b", "low", 40),
            mk("lower confidence", "c", "medium", 60),
        ],
        ..Default::default()
    };

    for text in [
        lines_text_concat(&render_todos_widget(&data, Rect::new(0, 0, 90, 10))),
        lines_text_concat(&render_todos_expanded(&data, Rect::new(0, 0, 90, 14))),
    ] {
        assert!(
            text.contains("high confidence 0/2 · confidence 85%"),
            "weighted group confidence missing: {text}"
        );
        assert!(
            text.contains("lower confidence 0/1 · confidence 60%"),
            "group-scoped confidence missing: {text}"
        );
    }
}

#[test]
fn todos_widgets_stay_flat_without_groups() {
    let mk = |id: &str, status: &str| crate::todo::TodoItem {
        group: None,
        id: id.to_string(),
        content: format!("task {id}"),
        status: status.to_string(),
        priority: "medium".to_string(),
        confidence: Some(80),
        completion_confidence: None,
        confidence_history: Vec::new(),
        blocked_by: Vec::new(),
        assigned_to: None,
    };
    let data = InfoWidgetData {
        todos: vec![mk("a", "completed"), mk("b", "pending")],
        ..Default::default()
    };
    let expanded = lines_text(&render_todos_expanded(&data, Rect::new(0, 0, 80, 14)));
    assert!(!expanded.contains("Other"), "no group bucket: {expanded}");
}

#[test]
fn todos_widget_renders_exact_pips_for_small_lists() {
    let mk = |status: &str| crate::todo::TodoItem {
        group: None,
        id: status.to_string(),
        content: format!("item {status}"),
        status: status.to_string(),
        priority: "medium".to_string(),
        confidence: Some(80),
        completion_confidence: None,
        confidence_history: Vec::new(),
        blocked_by: Vec::new(),
        assigned_to: None,
    };
    let data = InfoWidgetData {
        todos: vec![
            mk("completed"),
            mk("completed"),
            mk("in_progress"),
            mk("pending"),
        ],
        ..Default::default()
    };

    let lines = render_todos_widget(&data, Rect::new(0, 0, 80, 8));
    let header = lines_text(&lines[..1]);
    // Exact 1:1 pips on the header: 2 done + 1 active render as filled ●,
    // 1 open renders as hollow ○. (Active is full amber, not half.)
    assert_eq!(
        header.matches('●').count(),
        3,
        "expected 3 filled pips: {header}"
    );
    assert_eq!(
        header.matches('○').count(),
        1,
        "expected 1 open pip: {header}"
    );
    assert!(
        !header.contains('◐'),
        "active pip should be full, not half: {header}"
    );
    // The old block bar should be gone everywhere.
    let all = lines_text(&lines);
    assert!(!all.contains('█'), "old block bar should be gone: {all}");
    assert!(!all.contains('░'), "old empty bar should be gone: {all}");
}

fn plan_item(id: &str, status: &str) -> crate::plan::PlanItem {
    crate::plan::PlanItem {
        content: format!("task {id}"),
        status: status.to_string(),
        priority: "medium".to_string(),
        id: id.to_string(),
        subsystem: None,
        file_scope: Vec::new(),
        blocked_by: Vec::new(),
        assigned_to: None,
    }
}

#[test]
fn swarm_plan_todos_normalizes_scheduler_statuses() {
    let items = vec![
        plan_item("a", "running"),
        plan_item("b", "running_stale"),
        plan_item("c", "done"),
        plan_item("d", "completed"),
        plan_item("e", "failed"),
        plan_item("f", "stopped"),
        plan_item("g", "crashed"),
        plan_item("h", "queued"),
        plan_item("i", "ready"),
        plan_item("j", "blocked"),
        plan_item("k", "pending"),
        plan_item("l", "in_progress"),
        plan_item("m", "weird_custom_status"),
    ];
    let todos = swarm_plan_todos(&items);
    let status_of = |id: &str| {
        todos
            .iter()
            .find(|t| t.id == id)
            .map(|t| t.status.clone())
            .unwrap()
    };
    // Active scheduler states surface as in_progress (▶ amber, sorts first).
    assert_eq!(status_of("a"), "in_progress");
    assert_eq!(status_of("b"), "in_progress");
    // Terminal success maps onto completed (✓).
    assert_eq!(status_of("c"), "completed");
    assert_eq!(status_of("d"), "completed");
    // Terminal failure maps onto cancelled (✗) instead of an open circle.
    assert_eq!(status_of("e"), "cancelled");
    assert_eq!(status_of("f"), "cancelled");
    assert_eq!(status_of("g"), "cancelled");
    // Runnable / blocked states render as pending (○).
    assert_eq!(status_of("h"), "pending");
    assert_eq!(status_of("i"), "pending");
    assert_eq!(status_of("j"), "pending");
    // Statuses the todo renderer already understands pass through.
    assert_eq!(status_of("k"), "pending");
    assert_eq!(status_of("l"), "in_progress");
    // Arbitrary strings pass through unchanged (rendered as open ○).
    assert_eq!(status_of("m"), "weird_custom_status");
}

#[test]
fn swarm_plan_todos_preserve_blockers_and_assignee_and_flow_to_renderer() {
    let mut blocked = plan_item("audit-x", "queued");
    blocked.blocked_by = vec!["audit-y".to_string()];
    let mut running = plan_item("audit-y", "running");
    running.assigned_to = Some("worker-1".to_string());
    let items = vec![blocked, running];

    let todos = swarm_plan_todos(&items);
    assert_eq!(todos[0].blocked_by, vec!["audit-y".to_string()]);
    assert_eq!(todos[1].assigned_to.as_deref(), Some("worker-1"));

    let data = InfoWidgetData {
        todos,
        ..Default::default()
    };
    let text = lines_text(&render_todos_expanded(&data, Rect::new(0, 0, 80, 14)));
    // Blocked items get the dependency marker and suffix.
    assert!(text.contains("⊳"), "blocked glyph missing: {text}");
    assert!(text.contains("(blocked)"), "blocked suffix missing: {text}");
    // The running item sorts first as in_progress.
    let running_idx = text.find("task audit-y").unwrap();
    let blocked_idx = text.find("task audit-x").unwrap();
    assert!(running_idx < blocked_idx, "active-first order: {text}");
}

#[test]
fn swarm_plan_running_items_render_before_completed_in_large_plans() {
    // 120-item deep plan: 100 completed, 1 running near the end, rest queued.
    // The running item must be visible in the small line budget instead of
    // hiding behind the "+N more" footer.
    let mut items: Vec<crate::plan::PlanItem> = (0..100)
        .map(|i| plan_item(&format!("done-{i}"), "completed"))
        .collect();
    items.push(plan_item("hot-task", "running"));
    for i in 0..19 {
        items.push(plan_item(&format!("queued-{i}"), "queued"));
    }

    let data = InfoWidgetData {
        todos: swarm_plan_todos(&items),
        ..Default::default()
    };
    let text = lines_text(&render_todos_widget(&data, Rect::new(0, 0, 60, 8)));
    assert!(
        text.contains("task hot-task"),
        "running plan item should be visible in the budgeted list: {text}"
    );
    assert!(text.contains("+"), "footer summarizes the rest: {text}");
}

#[test]
fn todo_widget_header_says_plan_when_showing_swarm_plan_projection() {
    let items = vec![plan_item("a", "running"), plan_item("b", "queued")];
    let plan_data = InfoWidgetData {
        todos: swarm_plan_todos(&items),
        todos_are_swarm_plan: true,
        ..Default::default()
    };
    for text in [
        lines_text(&render_todos_widget(&plan_data, Rect::new(0, 0, 60, 8))),
        lines_text(&render_todos_expanded(&plan_data, Rect::new(0, 0, 60, 14))),
        lines_text(&render_todos_compact(&plan_data, Rect::new(0, 0, 60, 3))),
    ] {
        assert!(text.contains("Plan"), "plan header missing: {text}");
        assert!(!text.contains("Todos"), "plan must not claim Todos: {text}");
    }

    let todo_data = InfoWidgetData {
        todos: swarm_plan_todos(&items),
        todos_are_swarm_plan: false,
        ..Default::default()
    };
    let text = lines_text(&render_todos_widget(&todo_data, Rect::new(0, 0, 60, 8)));
    assert!(text.contains("Todos"), "todos header missing: {text}");
}

fn todo_item(id: &str, content: &str, status: &str, group: Option<&str>) -> crate::todo::TodoItem {
    crate::todo::TodoItem {
        content: content.to_string(),
        status: status.to_string(),
        priority: "medium".to_string(),
        id: id.to_string(),
        group: group.map(|g| g.to_string()),
        blocked_by: Vec::new(),
        assigned_to: None,
        confidence: Some(80),
        completion_confidence: None,
        confidence_history: Vec::new(),
    }
}

/// Join spans without separators so assertions can match text that spans
/// multiple styled segments (e.g. "hill " + "85%").
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
fn flat_todo_list_shows_hill_climbability_on_header_in_all_widget_sizes() {
    let data = InfoWidgetData {
        todos: vec![
            todo_item("a", "optimize grep", "in_progress", None),
            todo_item("b", "add bench", "pending", None),
        ],
        todo_goals: vec![crate::todo::TodoGoal {
            group: None,
            hill_climbability: Some(85),
            objective: Some("p50 under 50ms".to_string()),
            ..Default::default()
        }],
        ..Default::default()
    };
    for text in [
        lines_text_concat(&render_todos_widget(&data, Rect::new(0, 0, 70, 8))),
        lines_text_concat(&render_todos_expanded(&data, Rect::new(0, 0, 70, 14))),
        lines_text_concat(&render_todos_compact(&data, Rect::new(0, 0, 70, 3))),
    ] {
        assert!(text.contains("hill 85%"), "hill suffix missing: {text}");
    }
}

#[test]
fn grouped_todos_show_hill_climbability_on_their_group_headers() {
    let data = InfoWidgetData {
        todos: vec![
            todo_item("a", "speed up search", "in_progress", Some("optimize grep")),
            todo_item("b", "sketch layout", "pending", Some("onboarding design")),
        ],
        todo_goals: vec![
            crate::todo::TodoGoal {
                group: Some("optimize grep".to_string()),
                hill_climbability: Some(90),
                objective: Some("p50 under 50ms".to_string()),
                ..Default::default()
            },
            crate::todo::TodoGoal {
                group: Some("onboarding design".to_string()),
                hill_climbability: Some(20),
                objective: None,
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    for text in [
        lines_text_concat(&render_todos_widget(&data, Rect::new(0, 0, 70, 10))),
        lines_text_concat(&render_todos_expanded(&data, Rect::new(0, 0, 70, 14))),
    ] {
        assert!(text.contains("hill 90%"), "group hill missing: {text}");
        assert!(text.contains("hill 20%"), "low group hill missing: {text}");
    }
}

#[test]
fn todos_without_goals_render_no_hill_suffix() {
    let data = InfoWidgetData {
        todos: vec![todo_item("a", "do a thing", "pending", None)],
        ..Default::default()
    };
    for text in [
        lines_text_concat(&render_todos_widget(&data, Rect::new(0, 0, 70, 8))),
        lines_text_concat(&render_todos_expanded(&data, Rect::new(0, 0, 70, 14))),
        lines_text_concat(&render_todos_compact(&data, Rect::new(0, 0, 70, 3))),
    ] {
        assert!(!text.contains("hill"), "unexpected hill suffix: {text}");
    }
}

#[test]
fn hill_suffix_renders_safely_at_tiny_sizes() {
    let data = InfoWidgetData {
        todos: vec![todo_item(
            "a",
            "very long content that will need truncation 汉字 emoji 🚀",
            "in_progress",
            Some("a very long group name that must truncate"),
        )],
        todo_goals: vec![crate::todo::TodoGoal {
            group: Some("a very long group name that must truncate".to_string()),
            hill_climbability: Some(100),
            objective: None,
            ..Default::default()
        }],
        ..Default::default()
    };
    for (w, h) in [(0, 0), (1, 1), (5, 2), (12, 4), (200, 50)] {
        let rect = Rect::new(0, 0, w, h);
        let _ = render_todos_widget(&data, rect);
        let _ = render_todos_expanded(&data, rect);
        let _ = render_todos_compact(&data, rect);
    }
}

#[test]
fn swarm_plan_gate_items_render_like_normal_items() {
    // Deep-mode critique gates share the plan item shape; only the id differs.
    let mut gate = plan_item("explore-root::gate", "queued");
    gate.content = "Critique the work of 'explore-root' adversarially.".to_string();
    gate.blocked_by = vec!["explore-root".to_string()];
    let items = vec![plan_item("explore-root", "running"), gate];

    let data = InfoWidgetData {
        todos: swarm_plan_todos(&items),
        ..Default::default()
    };
    let text = lines_text(&render_todos_expanded(&data, Rect::new(0, 0, 80, 14)));
    assert!(text.contains("Critique the work"), "{text}");
    assert!(text.contains("(blocked)"), "gate blocked on parent: {text}");
}

#[test]
fn swarm_plan_todos_render_safely_at_extreme_sizes() {
    // Panic-safety sweep: long ids, wide glyphs, huge plans, tiny rects.
    let mut items: Vec<crate::plan::PlanItem> = (0..300)
        .map(|i| {
            let mut item = plan_item(
                &format!("very-long-node-id-{i}::gate::retry::{}", "x".repeat(80)),
                match i % 5 {
                    0 => "running",
                    1 => "completed",
                    2 => "failed",
                    3 => "queued",
                    _ => "blocked",
                },
            );
            item.content = format!("宽字符 emoji 🚀 test {} {}", i, "汉".repeat(40));
            item.blocked_by = vec!["dep".to_string()];
            item
        })
        .collect();
    items.push(plan_item("", ""));

    let data = InfoWidgetData {
        todos: swarm_plan_todos(&items),
        ..Default::default()
    };
    for (w, h) in [(0, 0), (1, 1), (2, 5), (7, 3), (20, 8), (200, 50)] {
        let rect = Rect::new(0, 0, w, h);
        let _ = render_todos_widget(&data, rect);
        let _ = render_todos_expanded(&data, rect);
        let _ = render_todos_compact(&data, rect);
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

    let expanded_text = lines_text(&render_usage_widget(&data, Rect::new(0, 0, 40, 4)));
    assert!(expanded_text.contains("$0.0123"));
    assert!(expanded_text.contains("12.3K in + 678 out"));

    let compact_text = lines_text(&render_usage_compact(&usage, 40));
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
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect::<Vec<_>>()
        .join("\n")
        .to_lowercase();

    assert!(text.contains("7 memories"));
    assert!(text.contains("find matches"));
    assert!(text.contains("check relevance"));
    assert!(text.contains("1/3"));
    assert!(text.contains("inject context"));
    assert!(text.contains("update memory"));
    assert!(text.contains("now:"));
    assert!(text.contains("checking 3 candidate"));
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
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect::<String>();

    assert!(text.contains("128 memories"), "{text}");
    for badge in [
        "IDLE", "SEARCH", "VERIFY", "READY", "INJECT", "SAVE", "UPDATE", "TOOL", "DONE", "FAILED",
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
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect::<Vec<_>>()
        .join("\n")
        .to_lowercase();

    assert!(text.contains("find matches"), "{text}");
    assert!(text.contains("check relevance"), "{text}");
    assert!(text.contains("inject context"), "{text}");
    assert!(text.contains("update memory"), "{text}");
    assert!(text.contains("checking 3 candidate"), "{text}");
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
    let one_section = InfoWidgetData {
        model: Some("gpt-test".to_string()),
        ..Default::default()
    };
    assert!(!one_section.has_data_for(WidgetKind::Overview));

    let two_sections = InfoWidgetData {
        model: Some("gpt-test".to_string()),
        queue_mode: Some(true),
        ..Default::default()
    };
    assert!(two_sections.has_data_for(WidgetKind::Overview));
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
        model: Some("gpt-test".to_string()),
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
        model: Some("gpt-test".to_string()),
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
    let lines = render_model_widget(&data, Rect::new(0, 0, 40, 10));
    let text = lines
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect::<Vec<_>>()
        .join("\n")
        .to_lowercase();
    assert!(text.contains("websocket"));
}

#[test]
fn usage_pill_renders_filled_and_empty_segments() {
    let line = super::render_usage_pill(200_000, 1_000_000, 26);
    let text: String = line
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect();

    assert!(text.contains('▰'), "expected filled pill segments: {text}");
    assert!(text.contains('▱'), "expected empty pill segments: {text}");
}

#[test]
fn usage_pill_renders_when_narrow() {
    let line = super::render_usage_pill(200_000, 1_000_000, 10);
    let text: String = line
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect();

    assert!(
        text.contains('▰') || text.contains('▱'),
        "narrow bar should still render pill segments: {text}"
    );
}

#[test]
fn context_usage_line_shows_numeric_label_inside_bar() {
    let line = super::render_context_usage_line("Context", 50_000, 200_000, 40);
    let text: String = line
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect();

    assert!(text.contains("Context"), "expected context label: {text}");
    assert!(
        text.contains("50k/200k"),
        "expected inline token label: {text}"
    );
}

#[test]
fn render_context_compact_prefers_observed_token_usage_for_label() {
    let data = InfoWidgetData {
        context_info: Some(crate::prompt::ContextInfo {
            total_chars: 400_000,
            ..Default::default()
        }),
        context_limit: Some(200_000),
        observed_context_tokens: Some(50_000),
        ..Default::default()
    };

    let lines = super::render_context_compact(&data, Rect::new(0, 0, 40, 1));
    let text: String = lines[0]
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect();

    assert!(
        text.contains("50k/200k"),
        "expected observed token count: {text}"
    );
    assert!(
        !text.contains("100k/200k"),
        "should not fall back to char estimate when observed tokens exist: {text}"
    );
}

#[test]
fn render_context_compact_reports_updating_when_snapshot_is_stale() {
    let data = InfoWidgetData {
        context_info_stale: true,
        context_info: Some(crate::prompt::ContextInfo {
            total_chars: 400_000,
            ..Default::default()
        }),
        context_limit: Some(200_000),
        ..Default::default()
    };

    let lines = super::render_context_compact(&data, Rect::new(0, 0, 40, 1));
    let text: String = lines[0]
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect();

    assert!(
        text.contains("updating"),
        "expected updating marker: {text}"
    );
    assert!(
        !text.contains("100k/200k"),
        "stale snapshots must not render old usage as current: {text}"
    );
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
/// mode: agents tally + node progress bar, and has_data_for admits the
/// widget into layout.
#[test]
fn swarm_widget_dock_mode_lists_managed_agents() {
    let data = InfoWidgetData {
        swarm_info: Some(SwarmInfo {
            managed_members: vec![
                managed_member("researcher", "running", Some("coordinator")),
                managed_member("reviewer", "completed", None),
            ],
            plan_progress: Some((3, 2, 7)),
            selected: 0,
            ..Default::default()
        }),
        ..Default::default()
    };
    assert!(data.has_data_for(WidgetKind::SwarmStatus));
    // Managing agents bumps the dock's effective priority near the top.
    assert!(data.effective_priority(WidgetKind::SwarmStatus) < WidgetKind::SwarmStatus.priority());

    let lines = super::render_swarm_widget(&data, Rect::new(0, 0, 34, 10));
    let text = lines_text(&lines);
    assert!(text.contains("1/2 agents"), "got: {text}");
    assert!(text.contains("nodes 3/7"), "got: {text}");
    // Second line is the plan progress bar (low-profile underline cells).
    assert_eq!(lines.len(), 2, "compact widget is exactly two lines");
    let bar: String = lines[1].spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(
        bar.chars().all(|c| c == '▁') && !bar.is_empty(),
        "expected underline bar cells: {bar}"
    );
    // Height: summary line + bar (+ borders).
    let h = calculate_widget_height(WidgetKind::SwarmStatus, &data, 34, 20);
    assert_eq!(h, 4, "compact height should be 2 content + 2 border: {h}");
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

    let text = lines_text(&super::render_swarm_widget(&data, Rect::new(0, 0, 80, 4)));

    assert!(text.contains("3s"), "got: {text}");
    assert!(text.contains("1c"), "got: {text}");
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
    let lines = super::render_swarm_widget(&data, Rect::new(0, 0, 40, 4));
    assert_eq!(lines.len(), 1, "expected only the stats line");
    // session_count == 0 and client_count == None: stats line is just the bee icon.
    let text = lines_text(&lines);
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

    let lines = super::render_swarm_widget(&data, Rect::new(0, 0, 30, 10));
    // Stats line + at most 3 member rows regardless of swarm size.
    assert_eq!(lines.len(), 4, "expected stats line + capped member rows");
    let text = lines_text(&lines);
    assert!(text.contains("500s"), "got: {text}");
    assert!(text.contains("3c"), "got: {text}");
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
    assert!(super::render_background_compact(&info).is_empty());

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
    let lines = super::render_background_widget(&data, Rect::new(0, 0, 40, 8));
    assert_eq!(lines.len(), 5, "summary + 3 tasks + overflow");
    let text = lines_text(&lines);
    assert!(text.contains("200 running"), "got: {text}");
    assert!(text.contains("+197 more"), "got: {text}");
    // Zero-size rect must not panic (row width clamps to a minimum).
    let _ = super::render_background_widget(&data, Rect::new(0, 0, 0, 0));
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
    };
    let data = InfoWidgetData {
        background_info: Some(info.clone()),
        ..Default::default()
    };

    let widget_text = lines_text(&super::render_background_widget(
        &data,
        Rect::new(0, 0, 40, 1),
    ));
    let compact_text = lines_text(&super::render_background_compact(&info));

    assert_eq!(widget_text, compact_text);
    assert!(widget_text.contains("Background"), "got: {widget_text}");
    assert!(widget_text.contains("4"), "got: {widget_text}");
    assert!(!widget_text.contains("mem:"), "got: {widget_text}");
    assert!(widget_text.contains("selfdev build"), "got: {widget_text}");
    assert!(widget_text.contains("train.py"), "got: {widget_text}");
    assert!(widget_text.contains("cargo test"), "got: {widget_text}");
    assert!(widget_text.contains("+1 more"), "got: {widget_text}");
    assert!(widget_text.contains("[#####-------]"), "got: {widget_text}");
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
        model: Some("gpt-test".to_string()),
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
        todos: vec![crate::todo::TodoItem {
            group: None,
            content: "ship patch".to_string(),
            status: "in_progress".to_string(),
            priority: "high".to_string(),
            id: "todo-1".to_string(),
            blocked_by: Vec::new(),
            assigned_to: None,
            confidence: None,
            completion_confidence: None,
            confidence_history: Vec::new(),
        }],
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

    // No todos/memory so the only candidate page is CompactOnly, and the
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

/// The fact ledger must mirror what each widget actually renders. The
/// Overview widget never draws the working directory, so it must not claim
/// Fact::Dir (that bug made the dir invisible everywhere: the HUD didn't show
/// it and the idle input hint believed it was already shown). Similarly, facts
/// backed by empty/blank values must not be claimed.
#[test]
fn claim_widget_facts_mirrors_rendering() {
    use crate::tui::session_facts::{Fact, FactLedger};

    let data = InfoWidgetData {
        model: Some("claude-fable-5".to_string()),
        reasoning_effort: Some("high".to_string()),
        provider_name: Some("claude".to_string()),
        auth_method: super::AuthMethod::AnthropicOAuth,
        working_dir: Some("/home/me/jcode".to_string()),
        session_count: Some(1),
        ..Default::default()
    };

    // Overview renders model/effort/provider/auth/session but NOT the dir.
    let mut ledger = FactLedger::new();
    super::claim_widget_facts(WidgetKind::Overview, &data, &mut ledger);
    assert!(ledger.is_shown(Fact::Model));
    assert!(ledger.is_shown(Fact::ReasoningEffort));
    assert!(ledger.is_shown(Fact::Provider));
    assert!(ledger.is_shown(Fact::Auth));
    assert!(ledger.is_shown(Fact::Session));
    assert!(
        ledger.is_missing(Fact::Dir),
        "Overview does not render the working dir; claiming it hides the dir everywhere"
    );
    assert!(
        ledger.is_missing(Fact::Context),
        "no context info in data, so Overview renders no context section"
    );

    // ModelInfo renders the dir line too.
    let mut ledger = FactLedger::new();
    super::claim_widget_facts(WidgetKind::ModelInfo, &data, &mut ledger);
    assert!(ledger.is_shown(Fact::Dir));

    // Blank / missing values must not be claimed even when the widget is placed.
    let blank = InfoWidgetData {
        model: Some("claude-fable-5".to_string()),
        reasoning_effort: Some("  ".to_string()),
        provider_name: Some(String::new()),
        working_dir: Some("   ".to_string()),
        session_name: Some(" ".to_string()),
        ..Default::default()
    };
    let mut ledger = FactLedger::new();
    super::claim_widget_facts(WidgetKind::ModelInfo, &blank, &mut ledger);
    assert!(ledger.is_shown(Fact::Model));
    assert!(ledger.is_missing(Fact::ReasoningEffort));
    assert!(ledger.is_missing(Fact::Provider));
    assert!(ledger.is_missing(Fact::Auth));
    assert!(ledger.is_missing(Fact::Dir));
    assert!(ledger.is_missing(Fact::Session));

    // A ModelInfo placement with no model renders nothing and claims nothing.
    let empty = InfoWidgetData::default();
    let mut ledger = FactLedger::new();
    super::claim_widget_facts(WidgetKind::ModelInfo, &empty, &mut ledger);
    assert!(ledger.is_missing(Fact::Model));

    // ContextUsage claims context only when there is real content.
    let mut ledger = FactLedger::new();
    super::claim_widget_facts(WidgetKind::ContextUsage, &empty, &mut ledger);
    assert!(ledger.is_missing(Fact::Context));
}
