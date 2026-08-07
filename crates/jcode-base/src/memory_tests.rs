use super::*;

#[test]
fn manager_without_project_dir_does_not_use_process_cwd() {
    assert!(MemoryManager::new().get_project_dir().is_none());
}
use crate::message::{ContentBlock, Message, Role};
use serde_json::json;
use std::fs;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

static PENDING_MEMORY_TEST_LOCK: Mutex<()> = Mutex::new(());

fn with_temp_home<F, T>(f: F) -> T
where
    F: FnOnce(&Path) -> T,
{
    let _guard = crate::storage::lock_test_env();
    let old = std::env::var("JCODE_HOME").ok();
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time went backwards")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("jcode-test-{}", unique));
    fs::create_dir_all(&dir).expect("create temp dir");
    crate::env::set_var("JCODE_HOME", &dir);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&dir)));

    match old {
        Some(value) => crate::env::set_var("JCODE_HOME", value),
        None => crate::env::remove_var("JCODE_HOME"),
    }
    let _ = fs::remove_dir_all(&dir);

    match result {
        Ok(value) => value,
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

#[test]
fn pending_memory_freshness_and_clear() {
    let _guard = PENDING_MEMORY_TEST_LOCK
        .lock()
        .expect("pending memory test lock poisoned");
    clear_all_pending_memory();

    let sid = "test-session-1";
    set_pending_memory(sid, "hello".to_string(), 2);
    assert!(has_pending_memory(sid));
    let pending = take_pending_memory(sid).expect("pending memory");
    assert_eq!(pending.prompt, "hello");
    assert_eq!(pending.count, 2);
    assert!(!has_pending_memory(sid));

    insert_pending_memory_for_test(
        sid,
        PendingMemory {
            prompt: "stale".to_string(),
            display_prompt: None,
            computed_at: Instant::now() - Duration::from_secs(121),
            count: 1,
            memory_ids: Vec::new(),
        },
    );
    assert!(take_pending_memory(sid).is_none());
}

#[test]
fn pending_memory_suppresses_immediate_duplicate_payloads() {
    let _guard = PENDING_MEMORY_TEST_LOCK
        .lock()
        .expect("pending memory test lock poisoned");
    clear_all_pending_memory();

    let sid = "test-session-2";
    set_pending_memory(sid, "same payload".to_string(), 1);
    assert!(take_pending_memory(sid).is_some());

    set_pending_memory(sid, "same payload".to_string(), 1);
    assert!(
        take_pending_memory(sid).is_none(),
        "identical payload should be suppressed when repeated immediately"
    );
}

#[test]
fn pending_memory_suppresses_overlapping_memory_sets() {
    let _guard = PENDING_MEMORY_TEST_LOCK
        .lock()
        .expect("pending memory test lock poisoned");
    clear_all_pending_memory();

    let sid = "test-session-overlap";
    set_pending_memory_with_ids(
        sid,
        "first payload".to_string(),
        2,
        vec!["mem-a".to_string(), "mem-b".to_string()],
    );
    assert!(take_pending_memory(sid).is_some());

    set_pending_memory_with_ids(
        sid,
        "second payload with same memories".to_string(),
        2,
        vec!["mem-b".to_string(), "mem-a".to_string()],
    );
    assert!(
        take_pending_memory(sid).is_none(),
        "same memory set should be suppressed even if prompt text differs"
    );
}

#[test]
fn pending_memory_keeps_existing_similar_payload_instead_of_replacing_it() {
    let _guard = PENDING_MEMORY_TEST_LOCK
        .lock()
        .expect("pending memory test lock poisoned");
    clear_all_pending_memory();

    let sid = "test-session-queued-overlap";
    set_pending_memory_with_ids(
        sid,
        "original payload".to_string(),
        2,
        vec!["mem-a".to_string(), "mem-b".to_string()],
    );
    set_pending_memory_with_ids(
        sid,
        "replacement payload".to_string(),
        2,
        vec!["mem-a".to_string(), "mem-b".to_string()],
    );

    let pending = take_pending_memory(sid).expect("existing pending payload should remain");
    assert_eq!(pending.prompt, "original payload");
}

#[test]
fn pending_memory_per_session_isolation() {
    let _guard = PENDING_MEMORY_TEST_LOCK
        .lock()
        .expect("pending memory test lock poisoned");
    clear_all_pending_memory();

    let sid_a = "test-session-a";
    let sid_b = "test-session-b";

    set_pending_memory(sid_a, "memory for A".to_string(), 1);
    set_pending_memory(sid_b, "memory for B".to_string(), 2);

    assert!(has_pending_memory(sid_a));
    assert!(has_pending_memory(sid_b));

    let pending_a = take_pending_memory(sid_a).expect("session A should have pending memory");
    assert_eq!(pending_a.prompt, "memory for A");
    assert!(!has_pending_memory(sid_a));

    // Session B's memory should still be there
    assert!(has_pending_memory(sid_b));
    let pending_b = take_pending_memory(sid_b).expect("session B should have pending memory");
    assert_eq!(pending_b.prompt, "memory for B");
    assert_eq!(pending_b.count, 2);
}

#[test]
fn pending_memory_suppresses_payload_when_all_ids_already_known() {
    let _guard = PENDING_MEMORY_TEST_LOCK
        .lock()
        .expect("pending memory test lock poisoned");
    clear_all_pending_memory();

    let sid = "test-session-known";
    mark_memories_known(sid, &["mem-x".to_string(), "mem-y".to_string()], "test");

    // Wait out the short-term signature/set suppression windows by using
    // distinct payload text; the id-level known check must trigger on its own.
    set_pending_memory_with_ids(
        sid,
        "brand new formatting of old knowledge".to_string(),
        2,
        vec!["mem-y".to_string(), "mem-x".to_string()],
    );
    assert!(
        take_pending_memory(sid).is_none(),
        "payload made entirely of already-known memories must not inject"
    );

    // A payload with at least one genuinely new memory still injects.
    set_pending_memory_with_ids(
        sid,
        "mix of old and new".to_string(),
        2,
        vec!["mem-x".to_string(), "mem-new".to_string()],
    );
    assert!(
        take_pending_memory(sid).is_some(),
        "payload containing an unknown memory should inject"
    );

    clear_all_pending_memory();
}

#[test]
fn injected_memory_dedup_expires_after_ttl() {
    let _guard = PENDING_MEMORY_TEST_LOCK
        .lock()
        .expect("pending memory test lock poisoned");
    clear_all_pending_memory();

    let sid = "test-session-ttl";
    mark_memories_injected(sid, &["mem-ttl".to_string()]);
    assert!(is_memory_injected(sid, "mem-ttl"));

    // Backdate past the TTL: the memory may surface again.
    backdate_injected_memory_for_test(sid, "mem-ttl", Duration::from_secs(46 * 60));
    assert!(
        !is_memory_injected(sid, "mem-ttl"),
        "injected-memory dedup must expire after the TTL"
    );
    assert!(!is_memory_injected_any("mem-ttl"));

    clear_all_pending_memory();
}

#[test]
fn mark_memories_known_blocks_reinjection_like_injection() {
    let _guard = PENDING_MEMORY_TEST_LOCK
        .lock()
        .expect("pending memory test lock poisoned");
    clear_all_pending_memory();

    let sid = "test-session-self-echo";
    let other = "test-session-other";

    // Simulates extraction: the memory came from sid's own transcript.
    mark_memories_known(sid, &["mem-echo".to_string()], "extracted from transcript");

    assert!(
        is_memory_injected(sid, "mem-echo"),
        "known memory must count as injected for its source session"
    );
    assert!(
        !is_memory_injected(other, "mem-echo"),
        "other sessions are unaffected by another session's known-marking"
    );

    clear_all_pending_memory();
}

#[test]
fn format_context_includes_roles_and_tools() {
    let messages = vec![
        Message::user("Hello world"),
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "tool-1".to_string(),
                name: "memory".to_string(),
                input: json!({"action": "list"}),
                thought_signature: None,
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
        Message::tool_result("tool-1", "ok", false),
        Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "tool-2".to_string(),
                content: "boom".to_string(),
                is_error: Some(true),
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
    ];

    let context = format_context_for_relevance(&messages);
    assert!(context.contains("User:\nHello world"));
    assert!(context.contains("[Tool: memory]"));
    assert!(!context.contains("[Tool result: ok]"));
    assert!(context.contains("[Tool error: boom]"));
}

#[test]
fn extraction_context_keeps_tool_io_details() {
    let messages = vec![
        Message::user("Hello world"),
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "tool-1".to_string(),
                name: "memory".to_string(),
                input: json!({"action": "list"}),
                thought_signature: None,
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
        Message::tool_result("tool-1", "ok", false),
    ];

    let context = format_context_for_extraction(&messages);
    assert!(context.contains("[Tool: memory input:"));
    assert!(context.contains("[Tool result: ok]"));
}

#[test]
fn memory_store_format_groups_by_category() {
    let mut store = MemoryStore::new();
    let now = Utc::now();
    let mut correction = MemoryEntry::new(MemoryCategory::Correction, "Fix lint rules");
    correction.updated_at = now;
    let mut fact = MemoryEntry::new(MemoryCategory::Fact, "Uses tokio");
    fact.updated_at = now;
    let mut preference = MemoryEntry::new(MemoryCategory::Preference, "Prefers ASCII-only edits");
    preference.updated_at = now;
    let mut entity = MemoryEntry::new(MemoryCategory::Entity, "Jeremy");
    entity.updated_at = now;
    let mut custom = MemoryEntry::new(MemoryCategory::Custom("team".to_string()), "Platform");
    custom.updated_at = now;

    store.add(correction);
    store.add(fact);
    store.add(preference);
    store.add(entity);
    store.add(custom);

    let output = store.format_for_prompt(10).expect("formatted output");
    let correction_idx = output.find("## Corrections").expect("correction heading");
    let fact_idx = output.find("## Facts").expect("fact heading");
    let preference_idx = output.find("## Preferences").expect("preference heading");
    let entity_idx = output.find("## Entities").expect("entity heading");
    let custom_idx = output.find("## team").expect("custom heading");

    assert!(correction_idx < fact_idx);
    assert!(fact_idx < preference_idx);
    assert!(preference_idx < entity_idx);
    assert!(entity_idx < custom_idx);
}

#[test]
fn memory_store_search_matches_content_and_tags() {
    let mut store = MemoryStore::new();
    let entry = MemoryEntry::new(MemoryCategory::Fact, "Uses Tokio runtime")
        .with_tags(vec!["async".to_string()]);
    store.add(entry);

    let content_hits = store.search("tokio");
    assert_eq!(content_hits.len(), 1);

    let tag_hits = store.search("ASYNC");
    assert_eq!(tag_hits.len(), 1);
}

#[test]
fn memory_search_normalizes_whitespace_and_separators() {
    let mut store = MemoryStore::new();
    let entry = MemoryEntry::new(MemoryCategory::Fact, "Uses side panel layout")
        .with_tags(vec!["build_cache".to_string()]);
    store.add(entry);

    assert_eq!(store.search("  side-panel  ").len(), 1);
    assert_eq!(store.search("BUILD.CACHE").len(), 1);
    assert!(store.search("   ").is_empty());
}

#[test]
fn manager_persists_and_forgets_memories() {
    with_temp_home(|_dir| {
        let manager = MemoryManager::new_test();
        let entry_project = MemoryEntry::new(MemoryCategory::Fact, "Project memory")
            .with_embedding(vec![1.0, 0.0, 0.0]);
        let entry_global = MemoryEntry::new(MemoryCategory::Preference, "Global memory")
            .with_embedding(vec![0.0, 1.0, 0.0]);

        let project_id = manager
            .remember_project(entry_project)
            .expect("remember project");
        let global_id = manager
            .remember_global(entry_global)
            .expect("remember global");

        let all = manager.list_all().expect("list all");
        assert_eq!(all.len(), 2);

        let search = manager.search("global").expect("search");
        assert_eq!(search.len(), 1);

        assert!(manager.forget(&project_id).expect("forget project"));
        let remaining = manager.list_all().expect("list all");
        assert_eq!(remaining.len(), 1);

        assert!(!manager.forget(&project_id).expect("forget missing"));
        assert!(manager.forget(&global_id).expect("forget global"));
    });
}

#[test]
fn graph_based_memory_operations() {
    with_temp_home(|_home| {
        let manager = MemoryManager::new_test();

        // Create two memories
        let entry1 = MemoryEntry::new(
            MemoryCategory::Fact,
            "The capital of France is Paris, a city known for the Eiffel Tower",
        );
        let entry2 = MemoryEntry::new(
            MemoryCategory::Fact,
            "Photosynthesis converts carbon dioxide and water into glucose using sunlight energy",
        );

        let id1 = manager.remember_project(entry1).expect("remember 1");
        let id2 = manager.remember_project(entry2).expect("remember 2");

        // Test tagging
        manager.tag_memory(&id1, "rust").expect("tag memory");
        manager.tag_memory(&id1, "language").expect("tag memory 2");
        manager.tag_memory(&id2, "rust").expect("tag memory 3");

        // Check graph stats (memories, tags, edges, clusters)
        let (mems, tags, edges, _clusters) = manager.graph_stats().expect("stats");
        assert_eq!(mems, 2, "expected 2 memories");
        assert_eq!(tags, 2, "expected 2 tags: rust and language");
        assert!(edges >= 3, "expected at least 3 edges, got {}", edges);

        // Test linking
        manager.link_memories(&id1, &id2, 0.8).expect("link");

        // Test get_related
        let related = manager.get_related(&id1, 2).expect("get related");
        assert!(!related.is_empty());
        // Should find id2 through the RelatesTo edge
        assert!(related.iter().any(|e| e.id == id2));

        // Clean up
        manager.forget(&id1).expect("forget 1");
        manager.forget(&id2).expect("forget 2");
    });
}

/// #729: test mode short-circuits `project_memory_path()` before the project
/// directory is consulted, so a manager in test mode cannot see real
/// project memory no matter what working directory it is given.
///
/// Swarm-spawned workers were forced into this mode unconditionally, which is
/// why they could never read what the session that spawned them remembered.
/// This pins the behavior so the severity of enabling test mode on a
/// production path stays visible.
#[test]
fn test_mode_ignores_the_project_dir_and_cannot_see_real_project_memory() {
    with_temp_home(|_home| {
        let project_dir = "/tmp/jcode-729-real-project";

        let real = MemoryManager::new().with_project_dir(project_dir);
        real.remember_project(MemoryEntry::new(
            MemoryCategory::Fact,
            "written by the spawning session",
        ))
        .expect("remember real project memory");

        // Same directory, but test mode: the write above is invisible.
        let isolated = MemoryManager::new_test().with_project_dir(project_dir);
        assert!(isolated.is_test_mode());
        let seen: Vec<String> = isolated
            .load_project_graph()
            .expect("load isolated graph")
            .all_memories()
            .map(|entry| entry.content.clone())
            .collect();
        assert!(
            !seen.iter().any(|c| c.contains("spawning session")),
            "test mode unexpectedly saw real project memory: {seen:?}"
        );

        // And a non-test manager on the same dir does see it, proving the
        // isolation above comes from test mode rather than a bad path.
        let reader = MemoryManager::new().with_project_dir(project_dir);
        let visible: Vec<String> = reader
            .load_project_graph()
            .expect("load real graph")
            .all_memories()
            .map(|entry| entry.content.clone())
            .collect();
        assert!(
            visible.iter().any(|c| c.contains("spawning session")),
            "real project memory should be visible without test mode: {visible:?}"
        );
    });
}

#[test]
fn project_memories_are_isolated_by_explicit_project_dir() {
    with_temp_home(|_home| {
        let manager_a = MemoryManager::new().with_project_dir("/tmp/jcode-project-a");
        let manager_b = MemoryManager::new().with_project_dir("/tmp/jcode-project-b");

        manager_a
            .remember_project(MemoryEntry::new(
                MemoryCategory::Fact,
                "memory from project a",
            ))
            .expect("remember project a");
        manager_b
            .remember_project(MemoryEntry::new(
                MemoryCategory::Fact,
                "memory from project b",
            ))
            .expect("remember project b");

        let project_a: Vec<String> = manager_a
            .load_project_graph()
            .expect("load project a")
            .all_memories()
            .map(|m| m.content.clone())
            .collect();
        let project_b: Vec<String> = manager_b
            .load_project_graph()
            .expect("load project b")
            .all_memories()
            .map(|m| m.content.clone())
            .collect();

        assert_eq!(project_a, vec!["memory from project a".to_string()]);
        assert_eq!(project_b, vec!["memory from project b".to_string()]);
    });
}

#[test]
fn manager_search_scoped_normalizes_whitespace_and_separators() {
    with_temp_home(|_home| {
        let manager = MemoryManager::new().with_project_dir("/tmp/jcode-search-normalization");

        manager
            .remember_project(MemoryEntry::new(
                MemoryCategory::Fact,
                "project compile notes",
            ))
            .expect("remember project");

        let hits = manager
            .search_scoped("  compile/notes  ", MemoryScope::Project)
            .expect("search project");
        assert_eq!(hits.len(), 1);
    });
}

#[test]
fn prompt_memories_scoped_keeps_only_most_recent_entries() {
    with_temp_home(|_home| {
        let manager = MemoryManager::new().with_project_dir("/tmp/jcode-prompt-topk");

        let mut oldest = MemoryEntry::new(MemoryCategory::Fact, "compile cache note");
        oldest.created_at = Utc::now() - chrono::Duration::seconds(30);
        oldest.updated_at = oldest.created_at;

        let mut middle = MemoryEntry::new(MemoryCategory::Fact, "oauth refresh bug");
        middle.created_at = Utc::now() - chrono::Duration::seconds(20);
        middle.updated_at = middle.created_at;

        let mut newest = MemoryEntry::new(MemoryCategory::Fact, "terminal shortcut hint");
        newest.created_at = Utc::now() - chrono::Duration::seconds(10);
        newest.updated_at = newest.created_at;

        manager
            .upsert_project_memory(oldest)
            .expect("remember oldest");
        manager
            .upsert_project_memory(middle)
            .expect("remember middle");
        manager
            .upsert_project_memory(newest)
            .expect("remember newest");

        let recent = manager
            .list_all_scoped(MemoryScope::Project)
            .expect("list project memories");
        assert_eq!(recent.len(), 3);
        assert_eq!(recent[0].content, "terminal shortcut hint");
        assert_eq!(recent[1].content, "oauth refresh bug");
        assert_eq!(recent[2].content, "compile cache note");

        let prompt = manager
            .get_prompt_memories_scoped(2, MemoryScope::Project)
            .expect("prompt memories");

        assert!(prompt.contains("terminal shortcut hint"));
        assert!(
            prompt.contains("oauth refresh bug") || prompt.contains("1.") || prompt.contains("2.")
        );
        assert!(!prompt.contains("compile cache note"));
    });
}

#[test]
fn goal_memory_upsert_skips_embedding_generation() {
    with_temp_home(|_home| {
        let manager = MemoryManager::new().with_project_dir("/tmp/jcode-goal-memory");

        let mut entry = MemoryEntry::new(
            MemoryCategory::Custom("goal".to_string()),
            "Goal: Ship mobile MVP\nStatus: active\nScope: project",
        );
        entry.id = "goal:ship-mobile-mvp".to_string();

        manager
            .upsert_project_memory(entry)
            .expect("upsert goal memory");

        let graph = manager.load_project_graph().expect("load graph");
        let saved = graph
            .get_memory("goal:ship-mobile-mvp")
            .expect("saved goal memory");
        assert!(
            saved.embedding.is_none(),
            "goal memory mirrors should not synchronously load/generate embeddings"
        );
    });
}

#[test]
fn scoped_retrieval_respects_project_vs_global() {
    with_temp_home(|_home| {
        let manager = MemoryManager::new().with_project_dir("/tmp/jcode-scope-test");

        manager
            .remember_project(MemoryEntry::new(
                MemoryCategory::Fact,
                "project zebra compile notes",
            ))
            .expect("remember project");
        manager
            .remember_global(MemoryEntry::new(
                MemoryCategory::Fact,
                "global coffee preference",
            ))
            .expect("remember global");

        let project = manager
            .list_all_scoped(MemoryScope::Project)
            .expect("list project");
        let global = manager
            .list_all_scoped(MemoryScope::Global)
            .expect("list global");
        let all = manager.list_all_scoped(MemoryScope::All).expect("list all");

        assert_eq!(project.len(), 1);
        assert_eq!(project[0].content, "project zebra compile notes");
        assert_eq!(global.len(), 1);
        assert_eq!(global[0].content, "global coffee preference");
        assert_eq!(all.len(), 2);

        let project_search = manager
            .search_scoped("zebra", MemoryScope::Project)
            .expect("search project");
        let global_search = manager
            .search_scoped("coffee", MemoryScope::Global)
            .expect("search global");

        assert_eq!(project_search.len(), 1);
        assert_eq!(project_search[0].content, "project zebra compile notes");
        assert_eq!(global_search.len(), 1);
        assert_eq!(global_search[0].content, "global coffee preference");
    });
}

#[test]
fn retrieval_candidates_include_local_skills() {
    with_temp_home(|home| {
        // memory no longer reaches into skill directly; register the skill
        // synthetic-entry provider (as cli::startup does in production) so the
        // memory<-skill integration this test exercises is wired up. The
        // shared snapshot is global-only (issue #457), so production composes
        // the process-cwd project overlay on top.
        crate::memory::register_synthetic_entry_provider(|| {
            let global = crate::skill::SkillRegistry::shared_snapshot();
            crate::skill::SkillRegistry::effective_for_working_dir(&global, None)
                .list()
                .into_iter()
                .map(|skill| skill.as_memory_entry())
                .collect()
        });
        let project_dir = home.join("project-with-skill");
        fs::create_dir_all(project_dir.join(".jcode/skills/firefox-browser"))
            .expect("create skills dir");
        fs::write(
                project_dir.join(".jcode/skills/firefox-browser/SKILL.md"),
                "---\nname: firefox-browser\ndescription: Control Firefox browser sessions\nallowed-tools: bash, read, write\n---\n\nUse this skill to open sites and click buttons.",
            )
            .expect("write skill");

        let old_cwd = std::env::current_dir().expect("current dir");
        std::env::set_current_dir(&project_dir).expect("set current dir");

        let manager = MemoryManager::new()
            .with_project_dir(&project_dir)
            .with_skills(true);
        let candidates = manager
            .collect_retrieval_candidates_scoped(MemoryScope::All)
            .expect("collect retrieval candidates");

        std::env::set_current_dir(old_cwd).expect("restore current dir");

        assert!(
            candidates
                .iter()
                .any(|entry| entry.id == "skill:firefox-browser")
        );
        assert!(candidates.iter().any(|entry| {
            matches!(
                entry.category,
                MemoryCategory::Custom(ref name) if name == "Skills"
            )
        }));
    });
}

#[test]
fn collect_skill_query_terms_keeps_relevant_words_and_drops_generic_words() {
    let terms = collect_skill_query_terms(
        "Before we start, make the todo list for this long debugging and validation task.",
    );

    assert!(terms.contains("todo"));
    assert!(terms.contains("debugging"));
    assert!(terms.contains("validation"));
    assert!(terms.contains("task"));
    assert!(!terms.contains("before"));
    assert!(!terms.contains("start"));
    assert!(!terms.contains("make"));
    assert!(!terms.contains("this"));
}

#[test]
fn score_and_filter_prioritizes_matching_skill_memories() {
    let generic = MemoryEntry::new(
        MemoryCategory::Fact,
        "General planning note that is not about structured todo skills.",
    )
    .with_embedding(vec![1.0, 0.0]);

    let mut skill = MemoryEntry::new(
        MemoryCategory::Custom("Skills".to_string()),
        "Use skill `/todo-planning-skill` for todo list planning, debugging, reflection, and validation on long tasks.",
    )
    .with_embedding(vec![1.0, 0.0])
    .with_source("skill_registry");
    skill.id = "skill:todo-planning-skill".to_string();

    let ranked = MemoryManager::score_and_filter(
        vec![generic, skill],
        &[1.0, 0.0],
        "Please make the todo list for this task.",
        0.0,
        2,
    )
    .expect("score and filter");

    assert_eq!(ranked.len(), 2);
    assert_eq!(ranked[0].0.id, "skill:todo-planning-skill");
    assert!(ranked[0].1 > ranked[1].1);
}

#[test]
fn hybrid_fuse_rescues_lexical_match_dense_would_miss() {
    // A memory that is the obvious lexical answer (shares the rare identifier
    // `find_similar_hybrid`) but is given a deliberately ORTHOGONAL embedding so
    // pure dense cosine ranks it last. BM25 must rescue it into the top result.
    let target = MemoryEntry::new(
        MemoryCategory::Fact,
        "The function find_similar_hybrid fuses dense and bm25 with RRF.",
    )
    .with_embedding(vec![0.0, 1.0]);

    let distractor_a = MemoryEntry::new(
        MemoryCategory::Fact,
        "Unrelated note about coffee brewing temperatures.",
    )
    .with_embedding(vec![1.0, 0.0]);
    let distractor_b = MemoryEntry::new(
        MemoryCategory::Fact,
        "Another unrelated note about bicycle maintenance.",
    )
    .with_embedding(vec![0.95, 0.05]);

    // Query embedding points along the distractors' axis, so dense alone would
    // rank the target dead last; the query TEXT contains the rare identifier.
    let query_text = "how does find_similar_hybrid work";
    let query_emb = vec![1.0, 0.0];

    let ranked = MemoryManager::hybrid_fuse(
        vec![target.clone(), distractor_a, distractor_b],
        query_text,
        &query_emb,
        3,
    );

    assert!(!ranked.is_empty(), "hybrid must return candidates");
    assert_eq!(
        ranked[0].0.id, target.id,
        "BM25 should rescue the exact-identifier memory to the top despite poor dense score"
    );
}

#[test]
fn hybrid_fuse_returns_dense_hits_without_lexical_overlap() {
    // When the query shares NO tokens with any memory, hybrid must still return
    // the dense-nearest memory (fusion falls back to the dense ranking).
    let near = MemoryEntry::new(MemoryCategory::Fact, "alpha bravo charlie")
        .with_embedding(vec![1.0, 0.0]);
    let far =
        MemoryEntry::new(MemoryCategory::Fact, "delta echo foxtrot").with_embedding(vec![0.0, 1.0]);

    let ranked = MemoryManager::hybrid_fuse(
        vec![near.clone(), far],
        "zzz_nonmatching_query_token",
        &[1.0, 0.0],
        2,
    );

    assert!(!ranked.is_empty());
    assert_eq!(
        ranked[0].0.id, near.id,
        "dense-nearest memory should rank first"
    );
}

#[test]
fn hybrid_excludes_superseded_memories() {
    with_temp_home(|_home| {
        let manager = MemoryManager::new().with_project_dir("/tmp/jcode-hybrid-supersede");

        // Two memories on the same topic with explicit distinct ids (avoid
        // same-millisecond id collisions).
        // Distinct embeddings so the write-time dedup does not merge them.
        let old = MemoryEntry::new(MemoryCategory::Fact, "The build uses cargo profile dev")
            .with_embedding(vec![1.0, 0.0]);
        let new = MemoryEntry::new(MemoryCategory::Fact, "The build uses cargo profile selfdev")
            .with_embedding(vec![0.0, 1.0]);

        let old_id = manager.remember_project(old).expect("remember old");
        let new_id = manager.remember_project(new).expect("remember new");
        assert_ne!(old_id, new_id, "ids must differ");

        // Supersede the old memory.
        let mut graph = manager.load_project_graph().expect("load");
        graph.supersede(&new_id, &old_id);
        manager.save_project_graph(&graph).expect("save");

        let results = manager
            .find_similar_hybrid("cargo build profile selfdev", &[0.0, 1.0], 10)
            .expect("hybrid");
        let ids: Vec<&str> = results.iter().map(|(e, _)| e.id.as_str()).collect();

        assert!(
            !ids.contains(&old_id.as_str()),
            "superseded memory must not surface from hybrid retrieval; got {:?}",
            ids
        );
        assert!(
            ids.contains(&new_id.as_str()),
            "the superseding memory should still surface; got {:?}",
            ids
        );
    });
}

#[test]
fn focus_query_text_strips_noise_and_leads_with_user_intent() {
    let raw = "\
<system-reminder>\n# Session Context\nDate: 2026-06-14\n</system-reminder>\n\
User:\n\
how do I fix the scroll bug in navigation.rs\n\
Assistant:\n\
Let me look at the handler.\n\
[Tool: read]\n\
[Result: fn handle_scroll() { ... }]\n\
Assistant:\n\
The bug is in the mouse delta calc.";

    let focused = super::focus_query_text(raw);

    // System-reminder block is gone.
    assert!(
        !focused.contains("Session Context"),
        "reminder not stripped: {focused}"
    );
    assert!(!focused.contains("<system-reminder>"));
    // Tool noise is gone.
    assert!(
        !focused.contains("[Tool:"),
        "tool marker not stripped: {focused}"
    );
    assert!(!focused.contains("[Result:"));
    // Role markers are gone.
    assert!(!focused.contains("User:"));
    assert!(!focused.contains("Assistant:"));
    // Real prose is kept.
    assert!(focused.contains("scroll bug in navigation.rs"));
    assert!(focused.contains("mouse delta calc"));
    // Leads with the latest user intent.
    assert!(
        focused.starts_with("how do I fix the scroll bug in navigation.rs"),
        "should lead with latest user message: {focused}"
    );
}

#[test]
fn focused_query_excludes_multiline_tool_errors_but_keeps_later_user_prose() {
    let messages = vec![Message {
        role: Role::User,
        content: vec![
            ContentBlock::ToolResult {
                tool_use_id: "tool-1".to_string(),
                content: "This command was not run.\nUNIQUE_MULTILINE_ERROR_PAYLOAD\nThe target cannot be confirmed.\nThe operation is irreversible."
                    .to_string(),
                is_error: Some(true),
            },
            ContentBlock::Text {
                text: "Keep the token rotation behavior unchanged.".to_string(),
                cache_control: None,
            },
        ],
        timestamp: None,
        tool_duration_ms: None,
    }];

    let focused = format_focused_query_for_relevance(&messages);

    assert!(!focused.contains("This command was not run"), "{focused}");
    assert!(
        !focused.contains("UNIQUE_MULTILINE_ERROR_PAYLOAD"),
        "arbitrary error payload leaked: {focused}"
    );
    assert!(!focused.contains("cannot be confirmed"), "{focused}");
    assert!(!focused.contains("irreversible"), "{focused}");
    assert!(
        focused.contains("Keep the token rotation behavior unchanged."),
        "subsequent user prose was lost: {focused}"
    );
}

#[test]
fn focus_query_text_falls_back_when_all_stripped() {
    let raw = "<system-reminder>\nonly boilerplate\n</system-reminder>\n[Tool: read]";
    let focused = super::focus_query_text(raw);
    // Nothing substantive survives -> fall back to raw rather than empty.
    assert_eq!(focused, raw);
}
