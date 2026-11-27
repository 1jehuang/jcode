use super::*;

#[test]
fn live_status_elapsed_and_tokens_invalidate_pinned_payload() {
    let mut agent = crate::tui::ui::TodoCardAgent {
        live_status: Some(crate::protocol::SwarmLiveStatus {
            phase: "running_tool".into(),
            elapsed_ms: 12_000,
            input_tokens: Some(123),
            ..Default::default()
        }),
        activity_age_secs: Some(1),
        ..Default::default()
    };
    let before = hash_todo_card_agents(&[agent.clone()]);
    advance_todo_agent_elapsed(&mut agent, std::time::Duration::from_secs(5));
    assert_eq!(agent.live_status.as_ref().unwrap().elapsed_ms, 17_000);
    assert_eq!(agent.activity_age_secs, Some(6));
    let later = hash_todo_card_agents(&[agent.clone()]);
    assert_ne!(before, later);
    agent.live_status.as_mut().unwrap().output_tokens = Some(456);
    assert_ne!(later, hash_todo_card_agents(&[agent]));
}

#[test]
fn shortcut_changes_invalidate_pinned_payload() {
    let mut agent = crate::tui::ui::TodoCardAgent {
        shortcut: Some(1),
        ..Default::default()
    };
    let before = hash_todo_card_agents(&[agent.clone()]);
    agent.shortcut = Some(0);
    assert_ne!(before, hash_todo_card_agents(&[agent]));
}

#[test]
fn worker_item_status_changes_invalidate_pinned_payload() {
    let mut agent = crate::tui::ui::TodoCardAgent {
        todo_statuses: vec!["pending".into(), "in_progress".into()],
        ..Default::default()
    };
    let before = hash_todo_card_agents(&[agent.clone()]);
    agent.todo_statuses.swap(0, 1);
    assert_ne!(before, hash_todo_card_agents(&[agent]));
}

#[test]
fn wait_and_activity_changes_invalidate_pinned_payload() {
    let mut agent = crate::tui::ui::TodoCardAgent::default();
    let before = hash_todo_card_agents(&[agent.clone()]);
    agent.wait_reason = Some("Waiting for build".into());
    let waiting = hash_todo_card_agents(&[agent.clone()]);
    assert_ne!(before, waiting);
    agent.wait_age_secs = Some(120);
    let aged = hash_todo_card_agents(&[agent.clone()]);
    assert_ne!(waiting, aged);
    agent.wait_age_secs = Some(125);
    assert_eq!(aged, hash_todo_card_agents(&[agent.clone()]));
    agent.activity_age_secs = Some(180);
    assert_ne!(aged, hash_todo_card_agents(&[agent.clone()]));
    agent.wait_reason = None;
    assert_ne!(waiting, hash_todo_card_agents(&[agent]));
}

/// Plan-level intent used by the view tests.
fn plan() -> crate::todo::TodoPlan {
    crate::todo::TodoPlan {
        user_intention: Some("make navigation feel immediate".to_string()),
        understands_user_intent: Some(crate::todo::IntentUnderstanding::Clear),
        ..Default::default()
    }
}

fn todo(
    id: &str,
    content: &str,
    status: &str,
    priority: &str,
    confidence: Option<crate::todo::ConfidenceState>,
    completion_confidence: Option<crate::todo::ConfidenceState>,
) -> TodoItem {
    TodoItem {
        id: id.to_string(),
        content: content.to_string(),
        status: status.to_string(),
        priority: priority.to_string(),
        group: None,
        lane_after: None,
        lane_owner: None,
        confidence,
        completion_confidence,
        confidence_history: Vec::new(),
        waiting_reason: None,
        requires_user_auth: None,
        waiting_on: Vec::new(),
        blocked_by: Vec::new(),
        assigned_to: None,
        stage: None,
        details: None,
    }
}

#[test]
fn complete_blocker_panel_preserves_reason_and_invalidates_cache() {
    let mut item = todo("1", "Connect", "pending", "high", None, None);
    let before = hash_todos_payload(None, &[item.clone()], &plan(), &[]);
    item.requires_user_auth = Some(crate::todo::TodoUserAuth {
        reason: "Only the account owner can grant access.\nKeep this entire explanation.".into(),
    });
    let after = hash_todos_payload(None, &[item.clone()], &plan(), &[]);
    assert_ne!(before, after);
    let markdown = format_todo_compact_markdown(&item);
    assert!(markdown.contains("✋"));
    assert!(markdown.contains("Needs your login"));
    assert!(!markdown.contains("Approve access"));
    assert!(markdown.contains("Keep this entire explanation."));
    item.requires_user_auth.as_mut().unwrap().reason.push('!');
    assert_ne!(
        after,
        hash_todos_payload(None, &[item.clone()], &plan(), &[])
    );
    for status in ["completed", "cancelled"] {
        item.status = status.into();
        assert!(!format_todo_compact_markdown(&item).contains("✋"));
    }
}

#[test]
fn todos_view_markdown_keeps_status_but_omits_internal_metadata() {
    let todos = vec![
        todo(
            "todo-1",
            "Validate confidence side panel",
            "in_progress",
            "high",
            Some(crate::todo::ConfidenceState::Plausible),
            None,
        ),
        todo(
            "todo-2",
            "Finish completed item",
            "completed",
            "medium",
            Some(crate::todo::ConfidenceState::Plausible),
            Some(crate::todo::ConfidenceState::Plausible),
        ),
    ];

    let markdown = build_todos_view_markdown(Some("session_test"), &todos, &plan(), &[]);

    assert!(markdown.contains("**1/2 completed** (50%) · 1 doing · 0 pending"));
    assert!(markdown.contains("- [doing] Validate confidence side panel"));
    assert!(markdown.contains("- [done] Finish completed item"));
    assert!(!markdown.contains("  - confidence:"));
    assert!(!markdown.contains("completion confidence:"));
    assert!(!markdown.contains("Session ID"));
    assert!(markdown.contains("Use `/todos` for full details."));
}

#[test]
fn todos_view_hash_changes_when_confidence_changes() {
    let mut todos = vec![todo(
        "todo-1",
        "Track confidence hash",
        "pending",
        "high",
        Some(crate::todo::ConfidenceState::Plausible),
        None,
    )];
    let before = hash_todos_payload(Some("session_test"), &todos, &plan(), &[]);
    todos[0].confidence = Some(crate::todo::ConfidenceState::Validated);
    let after = hash_todos_payload(Some("session_test"), &todos, &plan(), &[]);

    assert_ne!(before, after);
}

#[test]
fn todos_view_markdown_is_bounded_and_keeps_group_context() {
    let mut grouped_a = todo(
        "g1",
        "Cut frame allocs",
        "in_progress",
        "high",
        Some(crate::todo::ConfidenceState::Plausible),
        None,
    );
    grouped_a.group = Some("optimize rendering".to_string());
    let mut grouped_b = todo(
        "g2",
        "Batch draw calls",
        "completed",
        "medium",
        Some(crate::todo::ConfidenceState::Plausible),
        Some(crate::todo::ConfidenceState::Plausible),
    );
    grouped_b.group = Some("optimize rendering".to_string());
    let mut other = todo(
        "o1",
        "Fix scrollback",
        "pending",
        "low",
        Some(crate::todo::ConfidenceState::Plausible),
        None,
    );
    other.group = Some("scrollback".to_string());
    let ungrouped = todo(
        "u1",
        "Misc cleanup",
        "pending",
        "low",
        Some(crate::todo::ConfidenceState::Plausible),
        None,
    );

    let mut todos = vec![grouped_a, grouped_b, other, ungrouped];
    for index in 0..8 {
        todos.push(todo(
            &format!("extra-{index}"),
            &format!("Extra item {index}"),
            "pending",
            "low",
            Some(crate::todo::ConfidenceState::Plausible),
            None,
        ));
    }
    let markdown = build_todos_view_markdown(
        Some("session_test"),
        &todos,
        &plan(),
        &[crate::todo::TodoGoal {
            group: Some("optimize rendering".to_string()),
            closed_feedback_loop: Some(crate::todo::FeedbackLoopState::Strong),
            feedback_loop: Some("run the frame benchmark and compare p95 frame time".to_string()),
            feedback_loop_relevance: Some(crate::todo::FeedbackLoopRelevance::Representative),
            feedback_loop_coverage: Some(crate::todo::FeedbackLoopCoverage::MainPaths),
            delivery_state: Some(crate::todo::DeliveryState::WorkflowValidated),
            ..Default::default()
        }],
    );

    assert!(markdown.contains("_(optimize rendering)_"), "{markdown}");
    assert!(!markdown.contains("Feedback loop:"), "{markdown}");
    assert!(markdown.contains("_… and 4 more._"), "{markdown}");
    assert_eq!(markdown.matches("- [").count(), 8, "{markdown}");
}

#[test]
fn todos_view_hash_changes_when_group_changes() {
    let mut todos = vec![todo(
        "g",
        "Group hash",
        "pending",
        "high",
        Some(crate::todo::ConfidenceState::Plausible),
        None,
    )];
    let before = hash_todos_payload(Some("session_test"), &todos, &plan(), &[]);
    todos[0].group = Some("rendering".to_string());
    let after = hash_todos_payload(Some("session_test"), &todos, &plan(), &[]);
    assert_ne!(before, after);
}

#[test]
fn todos_view_hash_changes_when_goals_change() {
    let todos = vec![todo(
        "g",
        "Goal hash",
        "pending",
        "high",
        Some(crate::todo::ConfidenceState::Plausible),
        None,
    )];
    let before = hash_todos_payload(Some("session_test"), &todos, &plan(), &[]);
    let goals = vec![crate::todo::TodoGoal {
        closed_feedback_loop: Some(crate::todo::FeedbackLoopState::Weak),
        ..Default::default()
    }];
    let after = hash_todos_payload(Some("session_test"), &todos, &plan(), &goals);
    assert_ne!(before, after);
}

#[test]
fn todos_view_hash_changes_when_feedback_loop_changes() {
    let todos = vec![todo(
        "g",
        "Goal hash",
        "pending",
        "high",
        Some(crate::todo::ConfidenceState::Plausible),
        None,
    )];
    let mut goals = vec![crate::todo::TodoGoal {
        feedback_loop: Some("run test A".to_string()),
        ..Default::default()
    }];
    let before = hash_todos_payload(Some("session_test"), &todos, &plan(), &goals);
    goals[0].feedback_loop = Some("run test B".to_string());
    let after = hash_todos_payload(Some("session_test"), &todos, &plan(), &goals);
    assert_ne!(before, after);
}

#[test]
fn todos_view_hash_changes_when_user_intention_changes() {
    let todos = vec![todo(
        "g",
        "Goal hash",
        "pending",
        "high",
        Some(crate::todo::ConfidenceState::Plausible),
        None,
    )];
    let mut current = plan();
    let before = hash_todos_payload(Some("session_test"), &todos, &current, &[]);
    current.user_intention = Some("increase clarity".to_string());
    let after = hash_todos_payload(Some("session_test"), &todos, &current, &[]);
    assert_ne!(before, after);
}

#[test]
fn todos_view_hash_changes_when_intent_understanding_changes() {
    let todos = vec![todo(
        "g",
        "Goal hash",
        "pending",
        "high",
        Some(crate::todo::ConfidenceState::Plausible),
        None,
    )];
    let mut current = plan();
    let before = hash_todos_payload(Some("session_test"), &todos, &current, &[]);
    current.understands_user_intent = Some(crate::todo::IntentUnderstanding::Partial);
    let after = hash_todos_payload(Some("session_test"), &todos, &current, &[]);
    assert_ne!(before, after);
}
