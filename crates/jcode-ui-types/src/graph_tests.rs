use super::*;
use crate::{MemoryCategory, MemoryEntry, MemoryStore};

fn make_test_memory(content: &str) -> MemoryEntry {
    MemoryEntry::new(MemoryCategory::Fact, content)
}

#[test]
fn test_new_graph() {
    let graph = MemoryGraph::new();
    assert_eq!(graph.graph_version, GRAPH_VERSION);
    assert!(graph.memories.is_empty());
    assert!(graph.tags.is_empty());
}

#[test]
fn test_add_memory() {
    let mut graph = MemoryGraph::new();
    let entry = make_test_memory("Test content");
    let id = graph.add_memory(entry);

    assert!(graph.memories.contains_key(&id));
    assert_eq!(graph.get_memory(&id).unwrap().content, "Test content");
}

#[test]
fn test_add_memory_with_tags() {
    let mut graph = MemoryGraph::new();
    let entry = make_test_memory("Uses tokio").with_tags(vec!["rust".into(), "async".into()]);
    let id = graph.add_memory(entry);

    // Tags should be created
    assert!(graph.tags.contains_key("tag:rust"));
    assert!(graph.tags.contains_key("tag:async"));

    // Edges should exist
    let edges = graph.get_edges(&id);
    assert_eq!(edges.len(), 2);
    assert!(edges.iter().any(|e| e.target == "tag:rust"));
    assert!(edges.iter().any(|e| e.target == "tag:async"));
}

#[test]
fn test_tag_memory() {
    let mut graph = MemoryGraph::new();
    let entry = make_test_memory("Test");
    let id = graph.add_memory(entry);

    graph.tag_memory(&id, "newtag");

    assert!(graph.tags.contains_key("tag:newtag"));
    assert_eq!(graph.tags.get("tag:newtag").unwrap().count, 1);

    let memory = graph.get_memory(&id).unwrap();
    assert!(memory.tags.contains(&"newtag".to_string()));
}

#[test]
fn test_untag_memory() {
    let mut graph = MemoryGraph::new();
    let entry = make_test_memory("Test").with_tags(vec!["removeme".into()]);
    let id = graph.add_memory(entry);

    graph.untag_memory(&id, "removeme");

    let memory = graph.get_memory(&id).unwrap();
    assert!(!memory.tags.contains(&"removeme".to_string()));
    assert_eq!(graph.tags.get("tag:removeme").unwrap().count, 0);
}

#[test]
fn test_get_memories_by_tag() {
    let mut graph = MemoryGraph::new();

    let entry1 = make_test_memory("Memory 1").with_tags(vec!["shared".into()]);
    let entry2 = make_test_memory("Memory 2").with_tags(vec!["shared".into()]);
    let entry3 = make_test_memory("Memory 3").with_tags(vec!["other".into()]);

    graph.add_memory(entry1);
    graph.add_memory(entry2);
    graph.add_memory(entry3);

    let shared = graph.get_memories_by_tag("shared");
    assert_eq!(shared.len(), 2);

    let other = graph.get_memories_by_tag("other");
    assert_eq!(other.len(), 1);
}

#[test]
fn test_link_memories() {
    let mut graph = MemoryGraph::new();
    let id1 = graph.add_memory(make_test_memory("Memory A"));
    let id2 = graph.add_memory(make_test_memory("Memory B"));

    graph.link_memories(&id1, &id2, 0.8);

    let edges = graph.get_edges(&id1);
    assert!(
        edges.iter().any(|e| e.target == id2
            && matches!(e.kind, EdgeKind::RelatesTo { weight } if weight == 0.8))
    );
}

#[test]
fn test_supersede() {
    let mut graph = MemoryGraph::new();
    let old_id = graph.add_memory(make_test_memory("Old info"));
    let new_id = graph.add_memory(make_test_memory("New info"));

    graph.supersede(&new_id, &old_id);

    let old = graph.get_memory(&old_id).unwrap();
    assert!(!old.active);
    assert_eq!(old.superseded_by, Some(new_id.clone()));

    let edges = graph.get_edges(&new_id);
    assert!(
        edges
            .iter()
            .any(|e| e.target == old_id && matches!(e.kind, EdgeKind::Supersedes))
    );
}

#[test]
fn test_remove_memory() {
    let mut graph = MemoryGraph::new();
    let entry = make_test_memory("Test").with_tags(vec!["tag1".into()]);
    let id = graph.add_memory(entry);

    assert!(graph.memories.contains_key(&id));
    assert_eq!(graph.tags.get("tag:tag1").unwrap().count, 1);

    graph.remove_memory(&id);

    assert!(!graph.memories.contains_key(&id));
    assert_eq!(graph.tags.get("tag:tag1").unwrap().count, 0);
    assert!(graph.get_edges(&id).is_empty());
}

#[test]
fn test_node_and_edge_counts() {
    let mut graph = MemoryGraph::new();

    let entry1 = make_test_memory("M1").with_tags(vec!["t1".into()]);
    let entry2 = make_test_memory("M2").with_tags(vec!["t1".into(), "t2".into()]);

    graph.add_memory(entry1);
    graph.add_memory(entry2);

    // 2 memories + 2 tags = 4 nodes
    assert_eq!(graph.node_count(), 4);
    // M1->t1, M2->t1, M2->t2 = 3 edges
    assert_eq!(graph.edge_count(), 3);
}

#[test]
fn test_cascade_retrieval_through_tags() {
    let mut graph = MemoryGraph::new();

    // Create: A --HasTag--> tag:rust <--HasTag-- B
    //         A --HasTag--> tag:async <--HasTag-- C
    let id_a = graph
        .add_memory(make_test_memory("Memory A").with_tags(vec!["rust".into(), "async".into()]));
    let id_b = graph.add_memory(make_test_memory("Memory B").with_tags(vec!["rust".into()]));
    let id_c = graph.add_memory(make_test_memory("Memory C").with_tags(vec!["async".into()]));

    // Start from A with score 1.0
    let results = graph.cascade_retrieve(std::slice::from_ref(&id_a), &[1.0], 2, 10);

    // Should find A (seed), B (via rust tag), C (via async tag)
    assert!(results.iter().any(|(id, _)| id == &id_a));
    assert!(results.iter().any(|(id, _)| id == &id_b));
    assert!(results.iter().any(|(id, _)| id == &id_c));

    // A should have highest score (seed)
    let a_score = results
        .iter()
        .find(|(id, _)| id == &id_a)
        .map(|(_, s)| *s)
        .unwrap();
    let b_score = results
        .iter()
        .find(|(id, _)| id == &id_b)
        .map(|(_, s)| *s)
        .unwrap();
    assert!(a_score > b_score);
}

#[test]
fn test_cascade_retrieval_respects_result_limit_and_order() {
    let mut graph = MemoryGraph::new();

    let id_a = graph.add_memory(make_test_memory("Memory A"));
    let id_b = graph.add_memory(make_test_memory("Memory B"));
    let id_c = graph.add_memory(make_test_memory("Memory C"));
    let id_d = graph.add_memory(make_test_memory("Memory D"));

    graph.link_memories(&id_a, &id_b, 0.9);
    graph.link_memories(&id_a, &id_c, 0.8);
    graph.link_memories(&id_a, &id_d, 0.7);

    let results = graph.cascade_retrieve(std::slice::from_ref(&id_a), &[1.0], 1, 3);

    assert_eq!(results.len(), 3);
    assert_eq!(results[0].0, id_a);
    assert_eq!(results[1].0, id_b);
    assert_eq!(results[2].0, id_c);
    assert!(results[0].1 > results[1].1);
    assert!(results[1].1 > results[2].1);
}

#[test]
fn test_cascade_retrieval_respects_depth() {
    let mut graph = MemoryGraph::new();

    // Create chain: A --tag:t1--> B --tag:t2--> C --tag:t3--> D
    let id_a = graph.add_memory(make_test_memory("A").with_tags(vec!["t1".into()]));
    let id_b = graph.add_memory(make_test_memory("B").with_tags(vec!["t1".into(), "t2".into()]));
    let id_c = graph.add_memory(make_test_memory("C").with_tags(vec!["t2".into(), "t3".into()]));
    let _id_d = graph.add_memory(make_test_memory("D").with_tags(vec!["t3".into()]));

    // Depth 1: should find A, B (via t1)
    let results_d1 = graph.cascade_retrieve(std::slice::from_ref(&id_a), &[1.0], 1, 10);
    assert!(results_d1.iter().any(|(id, _)| id == &id_a));
    assert!(results_d1.iter().any(|(id, _)| id == &id_b));

    // Depth 2: should find A, B, C (via t1->t2)
    let results_d2 = graph.cascade_retrieve(std::slice::from_ref(&id_a), &[1.0], 2, 10);
    assert!(results_d2.iter().any(|(id, _)| id == &id_c));
}

#[test]
fn test_cascade_retrieval_via_relates_to() {
    let mut graph = MemoryGraph::new();

    let id_a = graph.add_memory(make_test_memory("Memory A"));
    let id_b = graph.add_memory(make_test_memory("Memory B"));
    let id_c = graph.add_memory(make_test_memory("Memory C"));

    // A --RelatesTo(0.8)--> B --RelatesTo(0.7)--> C
    graph.link_memories(&id_a, &id_b, 0.8);
    graph.link_memories(&id_b, &id_c, 0.7);

    let results = graph.cascade_retrieve(std::slice::from_ref(&id_a), &[1.0], 2, 10);

    // Should find all three
    assert!(results.iter().any(|(id, _)| id == &id_a));
    assert!(results.iter().any(|(id, _)| id == &id_b));
    assert!(results.iter().any(|(id, _)| id == &id_c));
}

#[test]
fn test_migration_from_legacy() {
    // Create a legacy MemoryStore
    let mut old_store = MemoryStore::new();
    old_store.add(make_test_memory("Memory 1").with_tags(vec!["tag1".into(), "tag2".into()]));
    old_store.add(make_test_memory("Memory 2").with_tags(vec!["tag1".into()]));

    // Migrate
    let graph = MemoryGraph::from_legacy_store(old_store);

    // Check version
    assert_eq!(graph.graph_version, GRAPH_VERSION);

    // Check memories migrated
    assert_eq!(graph.memories.len(), 2);

    // Check tags created
    assert!(graph.tags.contains_key("tag:tag1"));
    assert!(graph.tags.contains_key("tag:tag2"));
    assert_eq!(graph.tags.get("tag:tag1").unwrap().count, 2);
    assert_eq!(graph.tags.get("tag:tag2").unwrap().count, 1);

    // Check edges exist
    let edges_total: usize = graph.edges.values().map(|v| v.len()).sum();
    assert_eq!(edges_total, 3); // 2 edges for M1, 1 for M2
}

#[test]
fn test_graph_serialization_roundtrip() {
    let mut graph = MemoryGraph::new();

    // Add a memory with tags
    let entry = make_test_memory("Test memory").with_tags(vec!["rust".into()]);
    let id = graph.add_memory(entry);

    // Manually add a tag edge to verify serialization
    graph.tag_memory(&id, "extra");

    // Serialize
    let json = serde_json::to_string_pretty(&graph).expect("serialize");
    eprintln!("Serialized graph:\n{}", json);

    // Check edges appear in JSON
    assert!(json.contains("\"edges\""), "JSON should contain edges key");
    assert!(
        json.contains("tag:rust") || json.contains("tag:extra"),
        "JSON should contain tag references"
    );

    // Deserialize
    let parsed: MemoryGraph = serde_json::from_str(&json).expect("deserialize");

    // Verify
    assert_eq!(parsed.memories.len(), 1);
    assert_eq!(parsed.tags.len(), 2); // rust and extra
    assert_eq!(
        parsed.edge_count(),
        graph.edge_count(),
        "Edge count should match after roundtrip"
    );
}
