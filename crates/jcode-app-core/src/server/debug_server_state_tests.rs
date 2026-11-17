//! Server memory incident and compaction accounting regression tests.
use super::*;
use std::time::Duration;

#[test]
fn compaction_memory_totals_keep_lengths_separate_from_unique_capacities() {
    let mut totals = CompactionMemoryTotals::default();
    totals.observe(
        "canonical",
        &serde_json::json!({"compaction": {
            "summary_text_bytes": 10, "encrypted_content_bytes": 20,
            "retained_payload_capacity_bytes": 64,
            "persisted_metadata_payload_capacity_bytes": 0
        }}),
    );
    totals.observe(
        "metadata",
        &serde_json::json!({"compaction": {
            "summary_text_bytes": 3, "encrypted_content_bytes": 4,
            "retained_payload_capacity_bytes": 16,
            "persisted_metadata_payload_capacity_bytes": 128
        }}),
    );
    let result = totals.into_json();
    assert_eq!(result["summary_text_bytes"], 13);
    assert_eq!(result["encrypted_content_bytes"], 24);
    assert_eq!(result["retained_payload_capacity_bytes"], 80);
    assert_eq!(result["persisted_metadata_payload_capacity_bytes"], 128);
    assert_eq!(result["total_retained_payload_capacity_bytes"], 208);
    assert_eq!(
        result["top_by_retained_payload_capacity_bytes"][0]["session_id"],
        "metadata"
    );
}

#[test]
fn compaction_memory_totals_missing_fields_are_zero_and_top_is_bounded() {
    let mut totals = CompactionMemoryTotals::default();
    totals.observe("missing", &serde_json::json!({}));
    totals.observe(
        "old",
        &serde_json::json!({"compaction": {
            "summary_text_bytes": 5, "encrypted_content_bytes": 7
        }}),
    );
    for i in 1..=15u64 {
        totals.observe(
            &format!("session-{i}"),
            &serde_json::json!({"compaction": {
                "retained_payload_capacity_bytes": i
            }}),
        );
    }
    let result = totals.into_json();
    assert_eq!(result["summary_text_bytes"], 5);
    assert_eq!(result["encrypted_content_bytes"], 7);
    assert_eq!(result["total_retained_payload_capacity_bytes"], 120);
    assert_eq!(result["persisted_metadata_payload_capacity_bytes"], 0);
    let top = result["top_by_retained_payload_capacity_bytes"]
        .as_array()
        .unwrap();
    assert_eq!(top.len(), 12);
    assert_eq!(top[0]["retained_payload_capacity_bytes"], 15);
    assert_eq!(top[11]["retained_payload_capacity_bytes"], 4);
}

#[tokio::test]
async fn connected_session_snapshot_releases_connections_before_waiting_for_sessions() {
    let sessions = Arc::new(crate::server::session_manager::SessionManager::new());
    let client_connections = Arc::new(RwLock::new(HashMap::new()));
    let swarm_members = Arc::new(RwLock::new(HashMap::new()));

    let sessions_gate = sessions.write_for_test().await;
    let snapshot = connected_session_snapshot(&sessions, &client_connections, &swarm_members);
    tokio::pin!(snapshot);
    tokio::select! {
        _ = &mut snapshot => panic!("snapshot unexpectedly completed"),
        _ = tokio::time::sleep(Duration::from_millis(20)) => {}
    }

    let connections_guard =
        tokio::time::timeout(Duration::from_millis(100), client_connections.write())
            .await
            .expect("debug snapshot retained connections while waiting for sessions");
    drop(connections_guard);

    drop(sessions_gate);
    let (connected_agents, members) = tokio::time::timeout(Duration::from_secs(1), &mut snapshot)
        .await
        .expect("debug snapshot deadlocked");
    assert!(connected_agents.is_empty());
    assert!(members.is_empty());
}

#[test]
fn spawned_swarm_agent_count_only_includes_live_owned_sessions() {
    let live_session_ids = HashSet::from([
        "root".to_string(),
        "worker-running".to_string(),
        "worker-ready".to_string(),
    ]);
    let spawned_session_ids = [
        "worker-running".to_string(),
        "worker-ready".to_string(),
        "worker-stale".to_string(),
    ];

    assert_eq!(
        count_live_spawned_swarm_agents(&live_session_ids, spawned_session_ids.iter()),
        2
    );
}

#[test]
fn memory_incident_trend_handles_newest_first_and_reversed_history() {
    let mut history: Vec<_> = [(1_200_000, 900), (600_000, 500), (300_000, 200), (0, 100)]
        .into_iter()
        .map(
            |(timestamp_ms, pss_bytes)| crate::process_memory::ProcessMemoryHistoryEntry {
                timestamp_ms,
                source: "synthetic".to_string(),
                snapshot: crate::process_memory::ProcessMemorySnapshot {
                    os: Some(crate::process_memory::OsProcessMemoryInfo {
                        pss_bytes: Some(pss_bytes),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            },
        )
        .collect();
    for _ in 0..2 {
        let trend = memory_incident_trend(&history);
        assert_eq!(trend.current_timestamp_ms, 1_200_000);
        let baseline = trend.baseline.unwrap();
        assert_eq!(baseline.timestamp_ms, 300_000);
        let baseline_pss = baseline.snapshot.os.as_ref().unwrap().pss_bytes.unwrap();
        assert_eq!(900u64.saturating_sub(baseline_pss), 700);
        assert_eq!(trend.sample_count, 3);
        history.reverse();
    }
    // Arbitrary ordering must not change the baseline either.
    history.swap(0, 2);
    assert_eq!(
        memory_incident_trend(&history)
            .baseline
            .unwrap()
            .timestamp_ms,
        300_000
    );
}

#[test]
fn memory_incident_trend_uses_first_sample_inside_fifteen_minute_window() {
    let history: Vec<_> = [1_200_000, 299_999, 300_001, 900_000]
        .into_iter()
        .map(
            |timestamp_ms| crate::process_memory::ProcessMemoryHistoryEntry {
                timestamp_ms,
                source: "synthetic".to_string(),
                snapshot: Default::default(),
            },
        )
        .collect();
    let trend = memory_incident_trend(&history);
    assert_eq!(trend.baseline.unwrap().timestamp_ms, 300_001);
    assert_eq!(trend.sample_count, 3);
}

#[test]
fn memory_incident_trend_handles_empty_and_short_history() {
    let empty = memory_incident_trend(&[]);
    assert_eq!(empty.current_timestamp_ms, 0);
    assert!(empty.baseline.is_none());
    assert_eq!(empty.sample_count, 0);

    let history: Vec<_> = [60_000, 1_000]
        .into_iter()
        .map(
            |timestamp_ms| crate::process_memory::ProcessMemoryHistoryEntry {
                timestamp_ms,
                source: "synthetic".to_string(),
                snapshot: Default::default(),
            },
        )
        .collect();
    let trend = memory_incident_trend(&history);
    assert_eq!(trend.current_timestamp_ms, 60_000);
    assert_eq!(trend.baseline.unwrap().timestamp_ms, 1_000);
    assert_eq!(trend.sample_count, 2);
}

#[test]
fn memory_incident_classifies_runaway_live_sessions_before_allocator_retention() {
    let decision = classify_memory_incident(MemoryIncidentMetrics {
        pss_bytes: 4 * 1024 * 1024 * 1024,
        pss_growth_bytes: 3 * 1024 * 1024 * 1024,
        allocator_live_bytes: 3_800 * 1024 * 1024,
        allocator_retained_resident_bytes: 300 * 1024 * 1024,
        live_sessions: 1_145,
        headless_live_sessions: 1_140,
        connected_clients: 5,
    });

    assert_eq!(decision.severity, "critical");
    assert_eq!(decision.primary_cause, "runaway_live_session_population");
    assert_eq!(decision.confidence, "high");
}

#[test]
fn memory_incident_classifies_allocator_retention_when_live_heap_is_small() {
    let decision = classify_memory_incident(MemoryIncidentMetrics {
        pss_bytes: 1_500 * 1024 * 1024,
        pss_growth_bytes: 400 * 1024 * 1024,
        allocator_live_bytes: 500 * 1024 * 1024,
        allocator_retained_resident_bytes: 600 * 1024 * 1024,
        live_sessions: 8,
        headless_live_sessions: 3,
        connected_clients: 5,
    });

    assert_eq!(decision.severity, "warning");
    assert_eq!(decision.primary_cause, "allocator_retention");
    assert_eq!(decision.confidence, "high");
}

#[test]
fn memory_incident_reports_healthy_baseline() {
    let decision = classify_memory_incident(MemoryIncidentMetrics {
        pss_bytes: 220 * 1024 * 1024,
        pss_growth_bytes: 12 * 1024 * 1024,
        allocator_live_bytes: 150 * 1024 * 1024,
        allocator_retained_resident_bytes: 20 * 1024 * 1024,
        live_sessions: 5,
        headless_live_sessions: 1,
        connected_clients: 4,
    });

    assert_eq!(decision.severity, "healthy");
    assert_eq!(decision.primary_cause, "within_normal_operating_range");
}
