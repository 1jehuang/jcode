use super::*;
use crate::memory::MemoryCategory;

#[test]
fn extraction_transcript_omits_internal_system_reminders() {
    let messages = vec![
        crate::message::Message::user(
            "<system-reminder>\n# Session Context\nHardware: private\n</system-reminder>",
        ),
        crate::message::Message::user("Remember that tests use a temporary database."),
        crate::message::Message::assistant_text("Understood."),
    ];

    let transcript = build_transcript_for_extraction(&messages);

    assert!(!transcript.contains("Session Context"));
    assert!(!transcript.contains("Hardware: private"));
    assert!(transcript.contains("tests use a temporary database"));
    assert!(transcript.contains("Understood"));
}

#[test]
fn infer_candidate_tag_uses_repeated_non_stopword() {
    let tag =
        infer_candidate_tag("scheduler retries failed jobs and scheduler metrics update dashboard");
    assert_eq!(tag.as_deref(), Some("scheduler"));
}

#[test]
fn apply_cluster_assignment_links_members() {
    let mut graph = MemoryGraph::new();
    let mut a = MemoryEntry::new(MemoryCategory::Fact, "A");
    a.embedding = Some(vec![1.0, 0.0]);
    let id_a = graph.add_memory(a);

    let mut b = MemoryEntry::new(MemoryCategory::Fact, "B");
    b.embedding = Some(vec![0.0, 1.0]);
    let id_b = graph.add_memory(b);

    let stats = apply_cluster_assignment(
        &mut graph,
        "project",
        &[id_a.clone(), id_b.clone()],
        Utc::now(),
    );

    assert_eq!(stats.clusters_touched, 1);
    assert_eq!(stats.member_links, 2);
    assert_eq!(graph.clusters.len(), 1);

    let cluster_id = graph
        .clusters
        .keys()
        .next()
        .expect("cluster id")
        .to_string();
    assert!(
        graph
            .get_edges(&id_a)
            .iter()
            .any(|e| e.target == cluster_id && matches!(e.kind, EdgeKind::InCluster))
    );
    assert!(
        graph
            .get_edges(&id_b)
            .iter()
            .any(|e| e.target == cluster_id && matches!(e.kind, EdgeKind::InCluster))
    );
}

#[test]
fn apply_confidence_updates_batches_boost_and_decay() {
    let _guard = crate::storage::lock_test_env();
    let old = std::env::var("JCODE_HOME").ok();
    let dir = std::env::temp_dir().join(format!(
        "jcode-conf-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    crate::env::set_var("JCODE_HOME", &dir);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let manager = crate::memory::MemoryManager::new().with_project_dir("/tmp/jcode-conf-batch");

        let mut keep_entry = MemoryEntry::new(MemoryCategory::Fact, "verified memory")
            .with_embedding(vec![1.0, 0.0]);
        keep_entry.confidence = 0.5; // below cap so a boost is observable
        let keep = manager.remember_project(keep_entry).unwrap();
        let stale = manager
            .remember_project(
                MemoryEntry::new(MemoryCategory::Fact, "rejected memory")
                    .with_embedding(vec![0.0, 1.0]),
            )
            .unwrap();

        let conf_before = |id: &str| {
            manager
                .load_project_graph()
                .unwrap()
                .get_memory(id)
                .unwrap()
                .confidence
        };
        let keep_before = conf_before(&keep);
        let stale_before = conf_before(&stale);

        let (boosted, decayed) = apply_confidence_updates(
            &manager,
            std::slice::from_ref(&keep),
            std::slice::from_ref(&stale),
        );
        assert_eq!(boosted, 1, "one verified memory boosted");
        assert_eq!(decayed, 1, "one rejected memory decayed");

        let keep_after = conf_before(&keep);
        let stale_after = conf_before(&stale);
        assert!(keep_after > keep_before, "verified confidence should rise");
        assert!(
            stale_after < stale_before,
            "rejected confidence should fall"
        );
    }));

    match old {
        Some(v) => crate::env::set_var("JCODE_HOME", v),
        None => crate::env::remove_var("JCODE_HOME"),
    }
    let _ = std::fs::remove_dir_all(&dir);
    if let Err(p) = result {
        std::panic::resume_unwind(p);
    }
}

#[test]
fn should_run_rerank_cadence_and_overrides() {
    // First rerank of a session always fires.
    assert!(should_run_rerank(0, None, 3, false));
    assert!(should_run_rerank(5, None, 3, false));

    // Topic change always fires, even mid-cadence.
    assert!(should_run_rerank(4, Some(3), 3, true));

    // Cadence floor: with cadence=3, must wait 3 turns since last rerank.
    assert!(!should_run_rerank(4, Some(3), 3, false)); // 1 turn since -> gated
    assert!(!should_run_rerank(5, Some(3), 3, false)); // 2 turns since -> gated
    assert!(should_run_rerank(6, Some(3), 3, false)); // 3 turns since -> fire
    assert!(should_run_rerank(10, Some(3), 3, false)); // well past -> fire

    // cadence <= 1 disables gating (every turn fires).
    assert!(should_run_rerank(4, Some(3), 1, false));
    assert!(should_run_rerank(4, Some(3), 0, false));
}

#[test]
fn hybrid_retrieval_uses_focused_query_with_empty_fallback() {
    let context = "old session context and tool output";

    assert_eq!(
        retrieval_query(context, "current user question"),
        "current user question"
    );
    assert_eq!(retrieval_query(context, "  \n"), context);
}

fn mem(content: &str) -> MemoryEntry {
    MemoryEntry::new(MemoryCategory::Fact, content)
}

#[test]
fn dynamic_gate_cuts_tail_at_score_gap() {
    // RRF-style descending scores with a sharp gap after the second item.
    let cands = vec![
        (mem("a"), 0.0163_f32),
        (mem("b"), 0.0161),
        (mem("c"), 0.0100), // big drop -> tail cut here
        (mem("d"), 0.0098),
        (mem("e"), 0.0097),
    ];
    let out = dynamic_gate_select(cands, 5);
    assert_eq!(out.len(), 2, "should keep only the two close-scoring items");
    assert_eq!(out[0].0.content, "a");
    assert_eq!(out[1].0.content, "b");
}

#[test]
fn dynamic_gate_keeps_top1_even_when_isolated() {
    // A lone strong candidate followed by far-weaker ones: keep exactly 1.
    let cands = vec![
        (mem("a"), 0.0200_f32),
        (mem("b"), 0.0100),
        (mem("c"), 0.0090),
    ];
    let out = dynamic_gate_select(cands, 5);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].0.content, "a");
}

#[test]
fn dynamic_gate_respects_max_k_on_flat_scores() {
    // All scores ~equal: gate would keep all, but max_k caps the count.
    let cands: Vec<_> = (0..8)
        .map(|i| (mem(&format!("m{i}")), 0.0160_f32))
        .collect();
    let out = dynamic_gate_select(cands, 5);
    assert_eq!(out.len(), 5, "capped at max_k even when no gap appears");
}

#[test]
fn dynamic_gate_empty_input_returns_empty() {
    let out = dynamic_gate_select(Vec::new(), 5);
    assert!(out.is_empty());
}
