use super::*;

#[test]
fn default_await_members_targets_include_ready() {
    assert_eq!(
        default_await_target_statuses(),
        vec!["ready", "completed", "stopped", "failed", "crashed"]
    );
}

fn credential_failed_worker(session_id: &str, detail: &str, age_secs: u64) -> AgentInfo {
    AgentInfo {
        session_id: session_id.to_string(),
        status: Some("failed".to_string()),
        detail: Some(detail.to_string()),
        role: Some("agent".to_string()),
        is_headless: Some(true),
        report_back_to_session_id: Some("coord".to_string()),
        status_age_secs: Some(age_secs),
        provider_name: Some("anthropic".to_string()),
        ..Default::default()
    }
}

#[test]
fn credential_failure_wave_detected_for_recent_auth_failed_workers() {
    // The observed incident: every dispatched worker died within seconds with
    // an Anthropic 401 (expired OAuth + revoked refresh token) and nothing
    // completed. That must classify as a wave, not as N independent failures.
    let members = vec![
        AgentInfo {
            session_id: "coord".to_string(),
            status: Some("running".to_string()),
            role: Some("coordinator".to_string()),
            ..Default::default()
        },
        credential_failed_worker("w1", "Anthropic API error (401 Unauthorized)", 2),
        credential_failed_worker("w2", "Anthropic API error (401 Unauthorized)", 3),
        credential_failed_worker("w3", "invalid_grant: refresh token invalid", 5),
    ];
    let wave = super::super::detect_credential_failure_wave(&members, "coord", 0, 60)
        .expect("three recent credential failures with zero completions is a wave");
    assert_eq!(wave.session_ids, vec!["w1", "w2", "w3"]);
    assert_eq!(wave.sample_detail, "Anthropic API error (401 Unauthorized)");
    assert_eq!(wave.provider.as_deref(), Some("anthropic"));

    let message = super::super::format_credential_failure_wave_error(&wave, 60);
    assert!(message.contains("paused dispatching"));
    assert!(message.contains("3 worker(s)"));
    assert!(message.contains("401 Unauthorized"));
    assert!(message.contains("`jcode login --provider claude`"));
}

#[test]
fn credential_failure_wave_requires_at_least_two_workers() {
    let members = vec![credential_failed_worker(
        "w1",
        "Anthropic API error (401 Unauthorized)",
        2,
    )];
    assert_eq!(
        super::super::detect_credential_failure_wave(&members, "coord", 0, 60),
        None,
        "one bad worker is not a wave"
    );
}

#[test]
fn credential_failure_wave_not_detected_once_anything_completed() {
    // Completions prove the credential works (or worked); later auth failures
    // are then per-worker problems, not a route-wide outage to halt over.
    let members = vec![
        credential_failed_worker("w1", "Anthropic API error (401 Unauthorized)", 2),
        credential_failed_worker("w2", "Anthropic API error (401 Unauthorized)", 3),
    ];
    assert_eq!(
        super::super::detect_credential_failure_wave(&members, "coord", 1, 60),
        None
    );
}

#[test]
fn credential_failure_wave_ignores_stale_and_non_credential_failures() {
    let members = vec![
        // Stale: failed long before this window (e.g. a previous, already
        // diagnosed wave; the user has since re-authenticated and retried).
        credential_failed_worker("old", "Anthropic API error (401 Unauthorized)", 3600),
        // Unknown age must not count either.
        AgentInfo {
            status_age_secs: None,
            ..credential_failed_worker("ageless", "401 Unauthorized", 0)
        },
        // Non-credential failure.
        credential_failed_worker("crashed", "worker panicked: index out of bounds", 2),
        // Only one recent credential failure remains: below the wave minimum.
        credential_failed_worker("w1", "Anthropic API error (401 Unauthorized)", 2),
    ];
    assert_eq!(
        super::super::detect_credential_failure_wave(&members, "coord", 0, 60),
        None
    );
}

#[test]
fn credential_failure_wave_ignores_foreign_members() {
    // A foreign, client-attached session that failed with an auth error is not
    // one of run_plan's workers; it must not trip the breaker.
    let foreign = AgentInfo {
        is_headless: Some(false),
        report_back_to_session_id: None,
        ..credential_failed_worker("foreign", "401 Unauthorized", 2)
    };
    let members = vec![
        foreign,
        credential_failed_worker("w1", "401 Unauthorized", 2),
    ];
    assert_eq!(
        super::super::detect_credential_failure_wave(&members, "coord", 0, 60),
        None
    );
}

#[test]
fn credential_login_fix_hint_maps_provider_names() {
    assert_eq!(
        super::super::credential_login_fix_hint(Some("anthropic")),
        "`jcode login --provider claude`"
    );
    assert_eq!(
        super::super::credential_login_fix_hint(Some("OpenAI")),
        "`jcode login --provider openai`"
    );
    assert_eq!(
        super::super::credential_login_fix_hint(Some("copilot")),
        "`jcode login --provider copilot`"
    );
    assert_eq!(
        super::super::credential_login_fix_hint(None),
        "`jcode login --provider <provider>`"
    );
}

#[test]
fn run_plan_terminal_summary_includes_recorded_failure_reasons() {
    let mut failed_reasons = std::collections::BTreeMap::new();
    failed_reasons.insert(
        "c".to_string(),
        "task failed: Anthropic API error (401 Unauthorized)".to_string(),
    );
    let summary = crate::protocol::PlanGraphStatus {
        swarm_id: Some("swarm-a".to_string()),
        version: 1,
        item_count: 2,
        ready_ids: Vec::new(),
        blocked_ids: Vec::new(),
        active_ids: Vec::new(),
        completed_ids: vec!["a".to_string()],
        failed_ids: vec!["c".to_string()],
        failed_reasons,
        cycle_ids: Vec::new(),
        unresolved_dependency_ids: Vec::new(),
        next_ready_ids: Vec::new(),
        newly_ready_ids: Vec::new(),
        low_confidence_ids: Vec::new(),
        mode: "light".to_string(),
        seeded_count: 0,
        grown_count: 0,
    };
    let output = super::super::format_run_plan_terminal_summary(3, &summary, 2);
    assert!(output.contains("Failed nodes: c"));
    assert!(
        output.contains("c: task failed: Anthropic API error (401 Unauthorized)"),
        "terminal summary must carry the recorded failure reason:\n{output}"
    );

    let plan_status = format_plan_status(&summary).output;
    assert!(
        plan_status.contains("c: task failed: Anthropic API error (401 Unauthorized)"),
        "plan_status must display the recorded failure reason:\n{plan_status}"
    );
}
