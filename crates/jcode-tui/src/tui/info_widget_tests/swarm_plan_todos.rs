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
    let text = lines_text(&render_todos_widget(&data, Rect::new(0, 0, 60, 8)).all_lines());
    assert!(
        text.contains("task hot-task"),
        "running plan item should be visible in the budgeted list: {text}"
    );
    assert!(text.contains("+"), "footer summarizes the rest: {text}");
}
