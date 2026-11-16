use super::*;

#[test]
fn waiting_on_patch_supports_set_and_clear() {
    let initial = json!([{"id":"a", "content":"A", "status":"pending", "priority":"high", "waiting_on":["old"]}]);
    for waiting_on in [json!(["new"]), json!([]), Value::Null] {
        let input = apply_todo_patch(
            json!({"update":[{"id":"a", "waiting_on":waiting_on}]}),
            || initial.clone(),
        )
        .unwrap();
        let parsed = parse_todo_input(input).unwrap();
        let actual = &parsed.todos.unwrap()[0].waiting_on;
        assert_eq!(
            actual,
            &if waiting_on.is_null() {
                vec![]
            } else {
                serde_json::from_value::<Vec<String>>(waiting_on).unwrap()
            }
        );
    }
    let input = apply_todo_patch(json!({"add":[{"id":"b", "content":"B", "status":"pending", "priority":"high", "phase":1, "lane":"test", "waiting_on":["new"]}]}), || initial.clone()).unwrap();
    assert_eq!(
        parse_todo_input(input).unwrap().todos.unwrap()[1].waiting_on,
        vec!["new"]
    );
}

#[tokio::test]
async fn background_dependencies_validate_session_and_arm_wake() {
    let manager = crate::background::BackgroundTaskManager::new();
    let session = format!("todo-dependency-{}", uuid::Uuid::new_v4());
    let (release, wait) = tokio::sync::oneshot::channel::<()>();
    let task = manager
        .spawn_with_notify("test", None, &session, false, false, |_| async move {
            let _ = wait.await;
            Ok(crate::background::TaskResult::completed(Some(0)))
        })
        .await;
    let item = TodoItem {
        completely_blocked: None,
        external_blocker: None,
        waiting_on: vec![task.task_id.clone()],
        ..Default::default()
    };
    assert!(
        prepare_background_dependencies(
            &manager,
            "other-session",
            &[],
            std::slice::from_ref(&item)
        )
        .await
        .is_err()
    );
    let unknown = TodoItem {
        completely_blocked: None,
        external_blocker: None,
        waiting_on: vec!["missing-task".into()],
        ..Default::default()
    };
    assert!(
        prepare_background_dependencies(&manager, &session, &[], &[item.clone(), unknown])
            .await
            .is_err()
    );
    let status = manager.status(&task.task_id).await.unwrap();
    assert!(
        !status.notify && !status.wake,
        "invalid writes must not change delivery"
    );
    for status in ["completed", "cancelled"] {
        let mut closed = item.clone();
        closed.status = status.into();
        prepare_background_dependencies(&manager, &session, &[], &[closed])
            .await
            .unwrap();
        let delivery = manager.status(&task.task_id).await.unwrap();
        assert!(
            !delivery.notify && !delivery.wake,
            "closed todos must not arm wakes"
        );
    }
    let mut blocked = item.clone();
    blocked.completely_blocked = Some(crate::todo::TodoCompleteBlocker {
        reason: "Required data is permanently unavailable".into(),
    });
    prepare_background_dependencies(&manager, &session, &[], std::slice::from_ref(&blocked))
        .await
        .unwrap();
    let delivery = manager.status(&task.task_id).await.unwrap();
    assert!(
        !delivery.notify && !delivery.wake,
        "completely blocked todos must retain dependencies without arming delivery"
    );
    // A peer can still need the same task: the completely blocked condition must
    // not disable delivery for another actionable todo.
    prepare_background_dependencies(&manager, &session, &[], &[blocked, item.clone()])
        .await
        .unwrap();
    let status = manager.status(&task.task_id).await.unwrap();
    assert!(status.notify && status.wake);
    let mut events = Bus::global().subscribe();
    release.send(()).unwrap();
    let completion = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if let BusEvent::BackgroundTaskCompleted(event) = events.recv().await.unwrap()
                && event.task_id == task.task_id
            {
                break event;
            }
        }
    })
    .await
    .expect("completion must be delivered without a TUI");
    assert!(completion.notify && completion.wake);
    assert_eq!(
        manager.status(&task.task_id).await.unwrap().status,
        crate::bus::BackgroundTaskStatus::Completed
    );
    prepare_background_dependencies(&manager, &session, &[], &[item])
        .await
        .unwrap();
}

#[tokio::test]
async fn await_dependencies_validate_before_delivery_and_accept_terminal_race() {
    let _guard = crate::storage::lock_test_env();
    let key = format!("todo-await-{}", uuid::Uuid::new_v4());
    let session = "todo-await-owner";
    let id = crate::todo::await_dependency::dependency_id(&key, 1);
    let path = crate::storage::runtime_dir()
        .join(crate::todo::await_dependency::STATE_DIR)
        .join(format!("{key}.json"));
    let mut state = json!({
        "key": key, "session_id": session, "swarm_id": "swarm",
        "target_status": ["completed"], "requested_ids": ["worker"],
        "created_at_unix_ms": 1, "deadline_unix_ms": u64::MAX,
        "background": true, "notify": false, "wake": false, "final_response": null
    });
    crate::storage::write_json_fast(&path, &state).unwrap();
    let manager = crate::background::BackgroundTaskManager::new();
    let item = TodoItem {
        completely_blocked: None,
        external_blocker: None,
        waiting_on: vec![id.clone()],
        ..Default::default()
    };
    assert!(
        prepare_background_dependencies(&manager, "foreign", &[], std::slice::from_ref(&item))
            .await
            .is_err()
    );
    let invalid = TodoItem {
        completely_blocked: None,
        external_blocker: None,
        waiting_on: vec!["await-unknown-1".into()],
        ..Default::default()
    };
    assert!(
        prepare_background_dependencies(&manager, session, &[], &[item.clone(), invalid])
            .await
            .is_err()
    );
    let unchanged: Value = crate::storage::read_json(&path).unwrap();
    assert_eq!(unchanged["wake"], false);
    prepare_background_dependencies(&manager, session, &[], std::slice::from_ref(&item))
        .await
        .unwrap();
    let subscribed: Value = crate::storage::read_json(&path).unwrap();
    assert_eq!(subscribed["notify"], true);
    assert_eq!(subscribed["wake"], true);
    state["final_response"] = json!({"completed": true, "members": [], "summary": "done", "resolved_at_unix_ms": crate::todo::await_dependency::now_unix_ms()});
    crate::storage::write_json_fast(&path, &state).unwrap();
    prepare_background_dependencies(&manager, session, &[], std::slice::from_ref(&item))
        .await
        .unwrap();
    assert!(!crate::todo::await_dependency::running(
        &crate::storage::runtime_dir(),
        &id,
        session
    ));
    std::fs::remove_file(path).unwrap();
    // Historical IDs can remain on the same todo after state cleanup.
    prepare_background_dependencies(
        &manager,
        session,
        std::slice::from_ref(&item),
        std::slice::from_ref(&item),
    )
    .await
    .unwrap();
    assert!(
        prepare_background_dependencies(&manager, session, &[], &[item])
            .await
            .is_err()
    );
}

#[tokio::test]
async fn invalid_background_dependencies_do_not_write_state() {
    let _guard = crate::storage::lock_test_env();
    let previous_home = std::env::var_os("JCODE_HOME");
    let dir = tempfile::TempDir::new().unwrap();
    crate::env::set_var("JCODE_HOME", dir.path());
    let session = format!("todo-invalid-dependency-{}", uuid::Uuid::new_v4());
    let task = crate::background::global()
        .spawn("test", "different-session", |_| async {
            Ok(crate::background::TaskResult::completed(Some(0)))
        })
        .await;
    for id in [
        "missing-task",
        "",
        "../escape",
        "/absolute",
        "a/b",
        "a\\b",
        "a.b",
        task.task_id.as_str(),
    ] {
        let result = TodoTool::new().execute(json!({
            "todos":[{"id":"a", "content":"A", "status":"pending", "priority":"high", "waiting_on":[id]}],
            "plan":{"user_intention":"must not be saved"},
            "goals":[{"feedback_loop":"must not be saved"}]
        }), test_ctx(&session)).await;
        assert!(result.is_err());
        if id.contains(['/', '\\', '.']) {
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("Invalid waiting_on")
            );
        }
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            0,
            "rejected dependencies must not persist todos, assessments, or write statistics"
        );
    }
    if let Some(home) = previous_home {
        crate::env::set_var("JCODE_HOME", home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[tokio::test]
async fn missing_historical_dependencies_allow_unrelated_writes_but_not_new_refs() {
    let _guard = crate::storage::lock_test_env();
    let previous_home = std::env::var_os("JCODE_HOME");
    let dir = tempfile::TempDir::new().unwrap();
    crate::env::set_var("JCODE_HOME", dir.path());
    let session = "todo-retained-missing-background";
    let stored = vec![
        TodoItem {
            id: "old".into(),
            content: "old".into(),
            status: "completed".into(),
            priority: "high".into(),
            completely_blocked: None,
            external_blocker: None,
            waiting_on: vec!["removed-bg-task".into()],
            ..Default::default()
        },
        TodoItem {
            id: "other".into(),
            content: "other".into(),
            status: "pending".into(),
            priority: "high".into(),
            ..Default::default()
        },
    ];
    save_todos(session, &stored).unwrap();
    let tool = TodoTool::new();
    tool.execute(
        json!({"plan":{"user_intention":"assessment update"}}),
        test_ctx(session),
    )
    .await
    .unwrap();
    tool.execute(
        json!({"update":[{"id":"other", "status":"in_progress"}]}),
        test_ctx(session),
    )
    .await
    .unwrap();
    let before = load_todos(session).unwrap();
    assert_eq!(before[0].waiting_on, vec!["removed-bg-task"]);
    assert!(
        tool.execute(
            json!({"update":[{"id":"other", "waiting_on":["removed-bg-task"]}]}),
            test_ctx(session)
        )
        .await
        .is_err()
    );
    assert_eq!(load_todos(session).unwrap(), before);
    match previous_home {
        Some(home) => crate::env::set_var("JCODE_HOME", home),
        None => crate::env::remove_var("JCODE_HOME"),
    }
}

/// Issue #695, the visibly-stale case. The todos panel renders the
/// ungrouped goal unconditionally (not only as a group header), so an
/// ungrouped goal left over from a previous flat todo list is exactly what
/// the reporter saw frozen in the panel.
#[tokio::test]
async fn patch_mode_accepts_phase_and_lane_and_counts_write_modes() {
    let _guard = crate::storage::lock_test_env();
    let previous_home = std::env::var_os("JCODE_HOME");
    let dir = tempfile::TempDir::new().expect("tempdir");
    crate::env::set_var("JCODE_HOME", dir.path());
    let session = "todo-phase-lane";
    let tool = TodoTool::new();

    // Full write using the card's words.
    tool.execute(
        json!({"todos": [{"content": "a", "status": "pending", "priority": "high",
                          "id": "a", "confidence": "plausible",
                          "phase": 2, "lane": "parser"}]}),
        test_ctx(session),
    )
    .await
    .expect("full write with phase/lane");
    let todos = load_todos(session).unwrap();
    assert_eq!(todos[0].stage, Some(2));
    assert_eq!(todos[0].group.as_deref(), Some("parser"));

    // Patch moving it with the same words.
    tool.execute(
        json!({"update": [{"id": "a", "phase": 3, "lane": "ui"}]}),
        test_ctx(session),
    )
    .await
    .expect("patch with phase/lane");
    let todos = load_todos(session).unwrap();
    assert_eq!(todos[0].stage, Some(3));
    assert_eq!(todos[0].group.as_deref(), Some("ui"));

    tool.execute(json!({}), test_ctx(session))
        .await
        .expect("read");
    let counts: HashMap<String, u64> = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join("stats/todo-write-modes.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(counts.get("full"), Some(&1));
    assert_eq!(counts.get("patch"), Some(&1));
    assert_eq!(counts.get("read"), Some(&1));

    if let Some(home) = previous_home {
        crate::env::set_var("JCODE_HOME", home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[tokio::test]
async fn patch_mode_updates_adds_and_removes_without_resending_the_list() {
    let _guard = crate::storage::lock_test_env();
    let previous_home = std::env::var_os("JCODE_HOME");
    let dir = tempfile::TempDir::new().expect("tempdir");
    crate::env::set_var("JCODE_HOME", dir.path());
    let session = "todo-patch-mode";
    let tool = TodoTool::new();

    tool.execute(
        json!({"todos": [
            {"content": "a", "status": "pending", "priority": "high", "id": "a",
             "confidence": "plausible", "stage": 1, "group": "lane", "details": "keep me"},
            {"content": "b", "status": "pending", "priority": "high", "id": "b",
             "confidence": "plausible", "stage": 1, "group": "lane"},
        ]}),
        test_ctx(session),
    )
    .await
    .expect("initial write");

    // Status change by id: every other field and todo is untouched.
    tool.execute(
        json!({"update": [{"id": "a", "status": "completed",
                           "completion_confidence": "validated"}]}),
        test_ctx(session),
    )
    .await
    .expect("update");
    let todos = load_todos(session).unwrap();
    assert_eq!(todos[0].status, "completed");
    assert_eq!(todos[0].stage, Some(1));
    assert_eq!(todos[0].group.as_deref(), Some("lane"));
    assert_eq!(todos[0].details.as_deref(), Some("keep me"));
    assert_eq!(todos[1].status, "pending");

    // Insert after a given id, then remove one.
    tool.execute(
        json!({"add": [{"content": "between", "status": "pending", "priority": "high",
                        "id": "m", "confidence": "plausible", "after": "a"}],
               "remove": ["b"]}),
        test_ctx(session),
    )
    .await
    .expect("add + remove");
    let ids: Vec<String> = load_todos(session)
        .unwrap()
        .into_iter()
        .map(|t| t.id)
        .collect();
    assert_eq!(ids, ["a", "m"]);

    // Mistakes are rejected instead of silently doing something else.
    for bad in [
        json!({"update": [{"id": "nope", "status": "completed"}]}),
        json!({"add": [{"content": "dup", "status": "pending", "priority": "high",
                        "id": "a", "confidence": "plausible"}]}),
        json!({"update": [{"id": "a", "status": "finished-ish"}]}),
        json!({"todos": [], "update": [{"id": "a", "status": "pending"}]}),
    ] {
        assert!(
            tool.execute(bad.clone(), test_ctx(session)).await.is_err(),
            "{bad}"
        );
    }
    assert_eq!(
        load_todos(session).unwrap().len(),
        2,
        "failed patches change nothing"
    );

    if let Some(home) = previous_home {
        crate::env::set_var("JCODE_HOME", home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[tokio::test]
async fn an_ungrouped_goal_does_not_survive_into_a_grouped_next_task() {
    let _guard = crate::storage::lock_test_env();
    let previous_home = std::env::var_os("JCODE_HOME");
    let dir = tempfile::TempDir::new().expect("tempdir");
    crate::env::set_var("JCODE_HOME", dir.path());
    let session = "issue-695-ungrouped";
    let tool = TodoTool::new();

    // Task one: a flat (ungrouped) list, so its goal is the ungrouped one.
    tool.execute(
        json!({
            "todos": [{
                "content": "flat task one", "status": "in_progress",
                "priority": "high", "id": "t1", "confidence": 70,
            }],
            "plan": {"user_intention": "do task one", "understands_user_intent": 97},
            "goals": [{"closed_feedback_loop": 97, "feedback_loop": "ran the checks"}],
        }),
        test_ctx(session),
    )
    .await
    .expect("first write");
    let stored = load_goals(session).expect("goals");
    assert_eq!(stored.len(), 1);
    assert!(
        stored[0].group.is_none(),
        "task one goal is the ungrouped one"
    );

    // Task two: a grouped list. The ungrouped goal now describes nothing.
    tool.execute(
        json!({
            "todos": [{
                "content": "task two", "status": "in_progress", "priority": "high",
                "id": "t2", "group": "second task", "confidence": 70,
            }],
            "goals": [{"group": "second task", "closed_feedback_loop": 80,
                       "feedback_loop": "run the new checks"}],
        }),
        test_ctx(session),
    )
    .await
    .expect("second write");

    let goals = load_goals(session).expect("goals");
    assert!(
        !goals.iter().any(|goal| goal.group.is_none()),
        "the stale ungrouped goal must not stay in the panel: {goals:?}"
    );
    assert_eq!(goals.len(), 1);
    assert_eq!(goals[0].group.as_deref(), Some("second task"));

    if let Some(home) = previous_home {
        crate::env::set_var("JCODE_HOME", home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

/// Issue #695, end to end through the real tool: finish task one, then
/// start task two. What the todos panel renders (stored todos + goals) must
/// describe task two only, with no leftovers from task one.
#[tokio::test]
async fn moving_to_a_new_task_replaces_what_the_todos_panel_shows() {
    let _guard = crate::storage::lock_test_env();
    let previous_home = std::env::var_os("JCODE_HOME");
    let dir = tempfile::TempDir::new().expect("tempdir");
    crate::env::set_var("JCODE_HOME", dir.path());
    let session = "issue-695-new-task";
    let tool = TodoTool::new();

    // Task one, completed. `end_to_end_ownership` clears the completion
    // gate so the write is actually stored.
    tool.execute(
        json!({
            "todos": [{
                "content": "task one",
                "status": "completed",
                "priority": "high",
                "id": "t1",
                "group": "first task",
                "confidence": 90,
                "completion_confidence": 97,
            }],
            "plan": {"user_intention": "do task one", "understands_user_intent": 97},
            "goals": [{
                "group": "first task",
                "closed_feedback_loop": 97,
                "end_to_end_ownership": 97,
                "feedback_loop": "ran the checks",
            }],
        }),
        test_ctx(session),
    )
    .await
    .expect("first task write should succeed");
    assert_eq!(load_goals(session).expect("goals").len(), 1);

    // Task two: a fresh todo list in a new group.
    tool.execute(
        json!({
            "todos": [{
                "content": "task two",
                "status": "in_progress",
                "priority": "high",
                "id": "t2",
                "group": "second task",
                "confidence": 70,
            }],
            "goals": [{
                "group": "second task",
                "closed_feedback_loop": 80,
                "feedback_loop": "run the new checks",
            }],
        }),
        test_ctx(session),
    )
    .await
    .expect("second task write should succeed");

    let todos = load_todos(session).expect("todos");
    assert_eq!(todos.len(), 1, "panel must show only the current task");
    assert_eq!(todos[0].group.as_deref(), Some("second task"));

    let goals = load_goals(session).expect("goals");
    assert_eq!(
        goals.len(),
        1,
        "the finished task's goal must not linger in the panel: {goals:?}"
    );
    assert_eq!(goals[0].group.as_deref(), Some("second task"));

    if let Some(home) = previous_home {
        crate::env::set_var("JCODE_HOME", home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

/// End-to-end through the real tool, which is what the model actually sees.
/// A first plan write with honestly-moderate scores must come back clean:
/// this is the exact case that previously returned two nudges and spent the
/// turn re-justifying the plan instead of doing the work.
#[tokio::test]
async fn a_moderate_first_write_returns_no_continuation_and_records_instead() {
    let _guard = crate::storage::lock_test_env();
    let previous_home = std::env::var_os("JCODE_HOME");
    let dir = tempfile::TempDir::new().expect("tempdir");
    crate::env::set_var("JCODE_HOME", dir.path());
    let session = "gate-deferral-execute";

    let output = TodoTool::new()
        .execute(
            json!({
                "todos": [{
                    "content": "make utf16 transcode faster",
                    "status": "in_progress",
                    "priority": "high",
                    "id": "opt",
                    "group": "speed",
                    "confidence": 70,
                }],
                "plan": {
                    "user_intention": "beat the baseline",
                    "understands_user_intent": 82,
                },
                "goals": [{
                    "group": "speed",
                    "closed_feedback_loop": 80,
                    "feedback_loop": "run ./grade and read the score",
                    "feedback_loop_relevance": "indirect",
                    "feedback_loop_coverage": "narrow",
                }],
            }),
            test_ctx(session),
        )
        .await
        .expect("todo write should succeed");

    assert!(
        !output
            .output
            .contains(TODO_INTENT_UNDERSTANDING_CONTINUATION_MESSAGE),
        "a moderate first write must not be interrupted: {}",
        output.output
    );
    assert!(
        !output
            .output
            .to_ascii_lowercase()
            .contains("not high enough"),
        "no gate text should reach the model mid-turn: {}",
        output.output
    );

    // The points were recorded for the turn-end digest instead.
    let observations = crate::todo::load_gate_observations(session).expect("observations");
    assert_eq!(observations.len(), 5);
    assert!(
        observations
            .iter()
            .any(|observation| { observation.kind == GateObservationKind::FeedbackLoopRelevance })
    );
    assert!(
        observations
            .iter()
            .any(|observation| { observation.kind == GateObservationKind::FeedbackLoopCoverage })
    );
    assert!(
        observations.iter().any(|observation| {
            observation.kind == GateObservationKind::FeedbackLoopTraceability
        })
    );

    // Histories are accumulating, which is what the digest reasons over.
    let plan = load_plan(session).expect("plan");
    assert_eq!(
        plan.understands_user_intent_history,
        vec![crate::todo::IntentUnderstanding::Partial]
    );
    let goals = load_goals(session).expect("goals");
    assert_eq!(
        goals[0].closed_feedback_loop_history,
        vec![crate::todo::FeedbackLoopState::Strong]
    );
    assert_eq!(
        goals[0].feedback_loop_relevance_history,
        vec![crate::todo::FeedbackLoopRelevance::Indirect]
    );
    assert_eq!(
        goals[0].feedback_loop_coverage_history,
        vec![crate::todo::FeedbackLoopCoverage::Narrow]
    );

    // Second write at a higher score: still silent, history grows, and the
    // digest now has the trajectory available.
    let output = TodoTool::new()
        .execute(
            json!({"plan": {"understands_user_intent": 97}}),
            test_ctx(session),
        )
        .await
        .expect("second write should succeed");
    assert!(
        !output
            .output
            .to_ascii_lowercase()
            .contains("not high enough")
    );
    let plan = load_plan(session).expect("plan");
    assert_eq!(
        plan.understands_user_intent_history,
        vec![
            crate::todo::IntentUnderstanding::Partial,
            crate::todo::IntentUnderstanding::Clear
        ]
    );

    // The climb does not erase the point. The turn began without solid
    // understanding, so the work done before it settled still needs a
    // re-check; the wording just reflects that it settled late.
    let observations = crate::todo::load_gate_observations(session).expect("observations");
    let goals = load_goals(session).expect("goals");
    let digest = crate::todo::build_gate_digest(&observations, &plan, &goals)
        .expect("both recorded points should be surfaced");
    assert!(digest.contains("started this work without understanding"));
    assert!(digest.contains("feedback loop"));

    match previous_home {
        Some(value) => crate::env::set_var("JCODE_HOME", value),
        None => crate::env::remove_var("JCODE_HOME"),
    }
}

#[tokio::test]
async fn low_ownership_completion_is_saved_without_mid_write_rejection() {
    let _guard = crate::storage::lock_test_env();
    let previous_home = std::env::var_os("JCODE_HOME");
    let dir = tempfile::TempDir::new().expect("tempdir");
    crate::env::set_var("JCODE_HOME", dir.path());
    let session = "ownership-save-before-turn-gate";

    let output = TodoTool::new()
        .execute(
            json!({
                "todos": [{
                    "content": "ship the complete workflow",
                    "status": "completed",
                    "priority": "high",
                    "id": "ship",
                    "group": "release",
                    "confidence": 100,
                    "completion_confidence": 100,
                }],
                "goals": [{
                    "group": "release",
                    "closed_feedback_loop": 100,
                    "feedback_loop": "run the end-to-end release check",
                    "feedback_loop_relevance": "indirect",
                    "feedback_loop_coverage": "narrow",
                    "end_to_end_ownership": 95,
                }],
            }),
            test_ctx(session),
        )
        .await
        .expect("low ownership must not reject the todo write");

    let saved = load_todos(session).expect("completed todo should be persisted");
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].status, "completed");
    let saved_goals = load_goals(session).expect("goal should be persisted");
    let saved_goal = &saved_goals[0];
    assert_eq!(
        saved_goal.delivery_state,
        Some(crate::todo::DeliveryState::WorkflowValidated)
    );
    assert_eq!(
        saved_goal.feedback_loop_relevance,
        Some(crate::todo::FeedbackLoopRelevance::Indirect)
    );
    assert_eq!(
        saved_goal.feedback_loop_coverage,
        Some(crate::todo::FeedbackLoopCoverage::Narrow)
    );
    assert!(
        !output
            .output
            .contains(crate::todo::TODO_OWNERSHIP_CONTINUATION_MESSAGE),
        "ownership is enforced after the turn, not by rejecting the write: {}",
        output.output
    );

    match previous_home {
        Some(value) => crate::env::set_var("JCODE_HOME", value),
        None => crate::env::remove_var("JCODE_HOME"),
    }
}

#[path = "todo_quality_tests.rs"]
mod quality;
