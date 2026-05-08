#[test]
fn spawn_initial_message_accepts_prompt_alias_and_prefers_explicit_initial_message() {
    let from_prompt: CommunicateInput = serde_json::from_value(serde_json::json!({
        "action": "spawn",
        "prompt": "review the diff"
    }))
    .expect("prompt alias should deserialize");
    assert_eq!(
        from_prompt.spawn_initial_message().as_deref(),
        Some("review the diff")
    );

    let preferred: CommunicateInput = serde_json::from_value(serde_json::json!({
        "action": "spawn",
        "initial_message": "preferred",
        "prompt": "fallback"
    }))
    .expect("spawn payload should deserialize");
    assert_eq!(
        preferred.spawn_initial_message().as_deref(),
        Some("preferred")
    );
}

#[test]
fn communicate_input_accepts_delivery_and_share_append() {
    let delivery: CommunicateInput = serde_json::from_value(serde_json::json!({
        "action": "dm",
        "message": "ping",
        "to_session": "sess-2",
        "delivery": "wake"
    }))
    .expect("delivery mode should deserialize");
    assert_eq!(
        delivery.delivery,
        Some(crate::protocol::CommDeliveryMode::Wake)
    );

    let append: CommunicateInput = serde_json::from_value(serde_json::json!({
        "action": "share_append",
        "key": "task/123/notes",
        "value": "new line"
    }))
    .expect("share_append should deserialize");
    assert_eq!(append.action, "share_append");
}

#[test]
fn communicate_input_accepts_spawn_if_needed() {
    let parsed: CommunicateInput = serde_json::from_value(serde_json::json!({
        "action": "assign_task",
        "spawn_if_needed": true
    }))
    .expect("spawn_if_needed should deserialize");
    assert_eq!(parsed.spawn_if_needed, Some(true));
}

#[test]
fn communicate_input_accepts_prefer_spawn() {
    let parsed: CommunicateInput = serde_json::from_value(serde_json::json!({
        "action": "assign_task",
        "prefer_spawn": true
    }))
    .expect("prefer_spawn should deserialize");
    assert_eq!(parsed.prefer_spawn, Some(true));
}

#[test]
fn communicate_input_accepts_cleanup_lifecycle_flags() {
    let parsed: CommunicateInput = serde_json::from_value(serde_json::json!({
        "action": "run_plan",
        "force": true,
        "retain_agents": true
    }))
    .expect("lifecycle flags should deserialize");
    assert_eq!(parsed.force, Some(true));
    assert_eq!(parsed.retain_agents, Some(true));
}

#[test]
fn communicate_input_accepts_run_id() {
    let parsed: CommunicateInput = serde_json::from_value(serde_json::json!({
        "action": "run_plan",
        "run_id": "run-explicit-1"
    }))
    .expect("run_id should deserialize");
    assert_eq!(parsed.run_id.as_deref(), Some("run-explicit-1"));
}

#[test]
fn cleanup_candidates_default_to_owned_terminal_workers() {
    let members = vec![
        AgentInfo {
            session_id: "coord".to_string(),
            friendly_name: Some("coord".to_string()),
            files_touched: vec![],
            status: Some("ready".to_string()),
            detail: None,
            role: Some("coordinator".to_string()),
            is_headless: None,
            report_back_to_session_id: None,
            run_id: None,
            latest_completion_report: None,
            live_attachments: None,
            status_age_secs: None,
        },
        AgentInfo {
            session_id: "owned-done".to_string(),
            friendly_name: Some("owned".to_string()),
            files_touched: vec![],
            status: Some("completed".to_string()),
            detail: None,
            role: Some("agent".to_string()),
            is_headless: Some(true),
            report_back_to_session_id: Some("coord".to_string()),
            run_id: None,
            latest_completion_report: None,
            live_attachments: None,
            status_age_secs: None,
        },
        AgentInfo {
            session_id: "user-created".to_string(),
            friendly_name: Some("user".to_string()),
            files_touched: vec![],
            status: Some("completed".to_string()),
            detail: None,
            role: Some("agent".to_string()),
            is_headless: None,
            report_back_to_session_id: None,
            run_id: None,
            latest_completion_report: None,
            live_attachments: None,
            status_age_secs: None,
        },
        AgentInfo {
            session_id: "owned-running".to_string(),
            friendly_name: Some("running".to_string()),
            files_touched: vec![],
            status: Some("running".to_string()),
            detail: None,
            role: Some("agent".to_string()),
            is_headless: Some(true),
            report_back_to_session_id: Some("coord".to_string()),
            run_id: None,
            latest_completion_report: None,
            live_attachments: None,
            status_age_secs: None,
        },
    ];
    let statuses = default_cleanup_target_statuses();
    assert_eq!(
        cleanup_candidate_session_ids("coord", &members, &statuses, &[], false, None),
        vec!["owned-done".to_string()]
    );
    assert_eq!(
        cleanup_candidate_session_ids("coord", &members, &statuses, &[], true, None),
        vec!["owned-done".to_string(), "user-created".to_string()]
    );
}

#[test]
fn cleanup_candidates_include_owned_stale_workers_by_default() {
    let members = vec![
        AgentInfo {
            session_id: "coord".to_string(),
            friendly_name: Some("coord".to_string()),
            files_touched: vec![],
            status: Some("ready".to_string()),
            detail: None,
            role: Some("coordinator".to_string()),
            is_headless: None,
            report_back_to_session_id: None,
            run_id: None,
            latest_completion_report: None,
            live_attachments: None,
            status_age_secs: None,
        },
        AgentInfo {
            session_id: "owned-crashed".to_string(),
            friendly_name: Some("owned-crashed".to_string()),
            files_touched: vec![],
            status: Some("crashed".to_string()),
            detail: Some("recovered after reload".to_string()),
            role: Some("agent".to_string()),
            is_headless: Some(true),
            report_back_to_session_id: Some("coord".to_string()),
            run_id: Some("run-current".to_string()),
            latest_completion_report: None,
            live_attachments: None,
            status_age_secs: Some(900),
        },
        AgentInfo {
            session_id: "owned-running-stale".to_string(),
            friendly_name: Some("owned-stale".to_string()),
            files_touched: vec![],
            status: Some("running_stale".to_string()),
            detail: None,
            role: Some("agent".to_string()),
            is_headless: Some(true),
            report_back_to_session_id: Some("coord".to_string()),
            run_id: Some("run-current".to_string()),
            latest_completion_report: None,
            live_attachments: None,
            status_age_secs: Some(600),
        },
        AgentInfo {
            session_id: "foreign-crashed".to_string(),
            friendly_name: Some("foreign".to_string()),
            files_touched: vec![],
            status: Some("crashed".to_string()),
            detail: None,
            role: Some("agent".to_string()),
            is_headless: Some(true),
            report_back_to_session_id: Some("other-coord".to_string()),
            run_id: Some("run-foreign".to_string()),
            latest_completion_report: None,
            live_attachments: None,
            status_age_secs: Some(1200),
        },
    ];
    let statuses = default_cleanup_target_statuses();

    assert_eq!(
        cleanup_candidate_session_ids("coord", &members, &statuses, &[], false, None),
        vec![
            "owned-crashed".to_string(),
            "owned-running-stale".to_string()
        ]
    );
    assert_eq!(
        cleanup_candidate_session_ids("coord", &members, &statuses, &[], true, None),
        vec![
            "foreign-crashed".to_string(),
            "owned-crashed".to_string(),
            "owned-running-stale".to_string()
        ]
    );
}

#[test]
fn cleanup_candidates_can_be_scoped_by_run_id() {
    let members = vec![
        AgentInfo {
            session_id: "coord".to_string(),
            friendly_name: Some("coord".to_string()),
            files_touched: vec![],
            status: Some("ready".to_string()),
            detail: None,
            role: Some("coordinator".to_string()),
            is_headless: None,
            report_back_to_session_id: None,
            run_id: None,
            latest_completion_report: None,
            live_attachments: None,
            status_age_secs: None,
        },
        AgentInfo {
            session_id: "current-run".to_string(),
            friendly_name: Some("current".to_string()),
            files_touched: vec![],
            status: Some("completed".to_string()),
            detail: None,
            role: Some("agent".to_string()),
            is_headless: Some(true),
            report_back_to_session_id: Some("coord".to_string()),
            run_id: Some("run-current".to_string()),
            latest_completion_report: None,
            live_attachments: None,
            status_age_secs: None,
        },
        AgentInfo {
            session_id: "old-run".to_string(),
            friendly_name: Some("old".to_string()),
            files_touched: vec![],
            status: Some("completed".to_string()),
            detail: None,
            role: Some("agent".to_string()),
            is_headless: Some(true),
            report_back_to_session_id: Some("coord".to_string()),
            run_id: Some("run-old".to_string()),
            latest_completion_report: None,
            live_attachments: None,
            status_age_secs: None,
        },
    ];
    let statuses = default_cleanup_target_statuses();

    assert_eq!(
        cleanup_candidate_session_ids(
            "coord",
            &members,
            &statuses,
            &[],
            false,
            Some("run-current")
        ),
        vec!["current-run".to_string()]
    );
}

#[test]
fn format_swarm_health_summarizes_freshness_and_stale_members() {
    let ctx = test_ctx("coord", std::path::Path::new("."));
    let members = vec![
        AgentInfo {
            session_id: "coord".to_string(),
            friendly_name: Some("coord".to_string()),
            files_touched: vec![],
            status: Some("running".to_string()),
            detail: None,
            role: Some("coordinator".to_string()),
            is_headless: Some(false),
            report_back_to_session_id: None,
            run_id: None,
            latest_completion_report: None,
            live_attachments: Some(1),
            status_age_secs: Some(1),
        },
        AgentInfo {
            session_id: "owned-running".to_string(),
            friendly_name: Some("worker".to_string()),
            files_touched: vec![],
            status: Some("running".to_string()),
            detail: None,
            role: Some("agent".to_string()),
            is_headless: Some(true),
            report_back_to_session_id: Some("coord".to_string()),
            run_id: Some("run-current".to_string()),
            latest_completion_report: None,
            live_attachments: Some(0),
            status_age_secs: Some(3),
        },
        AgentInfo {
            session_id: "owned-ready".to_string(),
            friendly_name: Some("done".to_string()),
            files_touched: vec![],
            status: Some("ready".to_string()),
            detail: None,
            role: Some("agent".to_string()),
            is_headless: Some(true),
            report_back_to_session_id: Some("coord".to_string()),
            run_id: Some("run-current".to_string()),
            latest_completion_report: None,
            live_attachments: Some(0),
            status_age_secs: Some(10),
        },
        AgentInfo {
            session_id: "legacy".to_string(),
            friendly_name: Some("old-coord".to_string()),
            files_touched: vec![],
            status: Some("crashed".to_string()),
            detail: None,
            role: Some("coordinator".to_string()),
            is_headless: Some(false),
            report_back_to_session_id: None,
            run_id: None,
            latest_completion_report: None,
            live_attachments: Some(0),
            status_age_secs: Some(99),
        },
    ];

    let output = format_swarm_health(
        &ctx,
        std::path::Path::new("/tmp/jcode.sock"),
        &[1234],
        &members,
    )
    .output;

    assert!(output.contains("Swarm health"));
    assert!(output.contains("socket: /tmp/jcode.sock"));
    assert!(output.contains("server listener pid(s): 1234"));
    assert!(output.contains("members: total=4 owned=2 owned_active=1 owned_terminal=1 stale=1 foreign=1"));
    assert!(output.contains("statuses: crashed=1, ready=1, running=2"));
    assert!(output.contains("runs: run-current=2"));
    assert!(output.contains("scoped await default: 1 active owned candidate(s)"));
    assert!(output.contains("owned terminal members: done(ready)"));
    assert!(output.contains("stale members: old-coord(crashed)"));
}

#[test]
fn format_tool_summary_includes_call_count() {
    let output = super::format_tool_summary(
        "session-123",
        &[
            ToolCallSummary {
                tool_name: "read".to_string(),
                brief_output: "Read 20 lines".to_string(),
                timestamp_secs: None,
            },
            ToolCallSummary {
                tool_name: "grep".to_string(),
                brief_output: "Found 3 matches".to_string(),
                timestamp_secs: None,
            },
        ],
    );

    assert!(
        output
            .output
            .contains("Tool call summary for session-123 (2 calls):")
    );
    assert!(output.output.contains("read — Read 20 lines"));
    assert!(output.output.contains("grep — Found 3 matches"));
}

#[test]
fn format_members_includes_status_and_detail() {
    let ctx = ToolContext {
        session_id: "sess-self".to_string(),
        message_id: "msg-1".to_string(),
        tool_call_id: "call-1".to_string(),
        working_dir: None,
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: ToolExecutionMode::Direct,
    };

    let output = format_members(
        &ctx,
        &[AgentInfo {
            session_id: "sess-peer".to_string(),
            friendly_name: Some("bear".to_string()),
            files_touched: vec!["src/main.rs".to_string()],
            status: Some("running".to_string()),
            detail: Some("working on tests".to_string()),
            role: Some("agent".to_string()),
            is_headless: Some(true),
            report_back_to_session_id: Some("sess-self".to_string()),
            run_id: None,
            latest_completion_report: None,
            live_attachments: Some(0),
            status_age_secs: Some(12),
        }],
    );

    assert!(output.output.contains("Status: running — working on tests"));
    assert!(output.output.contains("Files: src/main.rs"));
    assert!(
        output
            .output
            .contains("Meta: headless · owned_by_you · attachments=0 · status_age=12s")
    );
}

#[test]
fn format_members_disambiguates_duplicate_friendly_names() {
    let ctx = test_ctx(
        "session_self_1234567890_deadbeefcafebabe",
        std::path::Path::new("."),
    );
    let output = format_members(
        &ctx,
        &[
            AgentInfo {
                session_id: "session_shark_1234567890_aaaaaaaaaaaa0001".to_string(),
                friendly_name: Some("shark".to_string()),
                files_touched: vec![],
                status: Some("ready".to_string()),
                detail: None,
                role: Some("agent".to_string()),
                is_headless: None,
                report_back_to_session_id: None,
                run_id: None,
                latest_completion_report: None,
                live_attachments: None,
                status_age_secs: None,
            },
            AgentInfo {
                session_id: "session_shark_1234567890_bbbbbbbbbbbb0002".to_string(),
                friendly_name: Some("shark".to_string()),
                files_touched: vec![],
                status: Some("ready".to_string()),
                detail: None,
                role: Some("agent".to_string()),
                is_headless: None,
                report_back_to_session_id: None,
                run_id: None,
                latest_completion_report: None,
                live_attachments: None,
                status_age_secs: None,
            },
        ],
    );

    assert!(output.output.contains("shark [aa0001]"));
    assert!(output.output.contains("shark [bb0002]"));
}

#[test]
fn format_awaited_members_disambiguates_duplicate_friendly_names() {
    let output = format_awaited_members(
        true,
        "done",
        &[
            AwaitedMemberStatus {
                session_id: "session_shark_1234567890_aaaaaaaaaaaa0001".to_string(),
                friendly_name: Some("shark".to_string()),
                status: "ready".to_string(),
                done: true,
                completion_report: None,
            },
            AwaitedMemberStatus {
                session_id: "session_shark_1234567890_bbbbbbbbbbbb0002".to_string(),
                friendly_name: Some("shark".to_string()),
                status: "ready".to_string(),
                done: true,
                completion_report: None,
            },
        ],
    );

    assert!(output.output.contains("✓ shark [aa0001] (ready)"));
    assert!(output.output.contains("✓ shark [bb0002] (ready)"));
}

#[test]
fn format_status_snapshot_includes_activity_and_metadata() {
    let output = super::format_status_snapshot(&AgentStatusSnapshot {
        session_id: "sess-peer".to_string(),
        friendly_name: Some("bear".to_string()),
        swarm_id: Some("swarm-test".to_string()),
        status: Some("running".to_string()),
        detail: Some("working on observability".to_string()),
        role: Some("agent".to_string()),
        is_headless: Some(true),
        live_attachments: Some(0),
        status_age_secs: Some(7),
        joined_age_secs: Some(42),
        files_touched: vec!["src/server/comm_sync.rs".to_string()],
        activity: Some(SessionActivitySnapshot {
            is_processing: true,
            current_tool_name: Some("bash".to_string()),
        }),
        provider_name: None,
        provider_model: None,
    });

    assert!(
        output
            .output
            .contains("Status snapshot for bear (sess-peer)")
    );
    assert!(
        output
            .output
            .contains("Lifecycle: running — working on observability")
    );
    assert!(output.output.contains("Activity: busy (bash)"));
    assert!(output.output.contains("Swarm: swarm-test"));
    assert!(
        output
            .output
            .contains("Meta: headless · attachments=0 · status_age=7s · joined=42s")
    );
    assert!(output.output.contains("Files: src/server/comm_sync.rs"));
}
