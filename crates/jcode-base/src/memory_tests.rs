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

pub(super) static PENDING_MEMORY_TEST_LOCK: Mutex<()> = Mutex::new(());

/// Serializes tests that read-then-assert deltas on the process-wide
/// slot-B shadow counters (`PREFILTER_FAILOPEN_ARMS`, stage accumulators).
/// Rust runs tests in parallel threads; without this, one test's bump
/// lands between another test's before/after reads and exact-delta
/// assertions flake (observed: +2 instead of +1 on arm slots).
static PREFILTER_SHADOW_TEST_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn jev_storage_preserves_code_spelling_and_scope_without_embeddings() {
    with_temp_home(|_| {
        let manager = MemoryManager::new().with_project_dir("/jev-storage");
        let first = manager
            .remember_project(MemoryEntry::new(MemoryCategory::Fact, "path: foo/bar"))
            .unwrap();
        let second = manager
            .remember_project(MemoryEntry::new(MemoryCategory::Fact, "path: foo-bar"))
            .unwrap();
        let third = manager
            .remember_project(MemoryEntry::new(MemoryCategory::Fact, "path: Foo/bar"))
            .unwrap();
        assert_ne!(first, second);
        assert_ne!(first, third);
        let repeated = manager
            .remember_project(MemoryEntry::new(MemoryCategory::Fact, "path: foo/bar"))
            .unwrap();
        assert_eq!(first, repeated);
        let global = manager
            .remember_global(MemoryEntry::new(MemoryCategory::Fact, "path: foo/bar"))
            .unwrap();
        assert_ne!(first, global);
        let all = manager.list_all().unwrap();
        assert_eq!(all.len(), 4);
        assert!(all.iter().all(|entry| entry.embedding.is_none()));
        assert_eq!(
            manager
                .load_project_graph()
                .unwrap()
                .get_memory(&first)
                .unwrap()
                .strength,
            2
        );
    });
}

#[test]
fn jev_project_write_without_scope_fails_instead_of_silently_losing_memory() {
    with_temp_home(|_| {
        let manager = MemoryManager::new();
        assert!(
            manager
                .remember_project(MemoryEntry::new(MemoryCategory::Fact, "fact"))
                .is_err()
        );
        assert!(
            manager
                .remember_global(MemoryEntry::new(MemoryCategory::Fact, "fact"))
                .is_ok()
        );
    });
}

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

#[test]
fn jev_recall_prefilter_bounds_candidates_and_keeps_relevant_memories() {
    let mut entries: Vec<MemoryEntry> = (0..2000)
        .map(|i| {
            MemoryEntry::new(
                MemoryCategory::Fact,
                format!("unrelated note number {i} about gardening"),
            )
        })
        .collect();
    entries.push(MemoryEntry::new(
        MemoryCategory::Fact,
        "The fundraising CRM lives on Bookface and uses a 40M post-money SAFE cap",
    ));
    let kept = prefilter_for_jev(entries, "update the fundraising CRM notes for the SAFE");
    assert!(kept.len() <= MAX_JEV_RECALL_CANDIDATES);
    assert!(kept.iter().any(|e| e.content.contains("fundraising CRM")));
    // At most three Jev batches of 24 per recall instead of ~84 for this store.
    assert!(kept.len().div_ceil(crate::memory_jev::MAX_BATCH_ENTRIES) <= 3);
}

#[test]
fn jev_recall_prefilter_leaves_small_stores_untouched() {
    let entries: Vec<MemoryEntry> = (0..10)
        .map(|i| MemoryEntry::new(MemoryCategory::Fact, format!("note {i}")))
        .collect();
    assert_eq!(
        prefilter_for_jev(entries, "completely different words").len(),
        10
    );
}

#[test]
fn bm25_plural_fold_makes_tied_pair_score_equal() {
    // c2b mechanism: a tied pair differing only in plurality must score
    // equal at the bm25_rank level (symmetric folding on query AND docs).
    let mk = |content: &str| {
        MemoryEntry::new(MemoryCategory::Fact, content)
    };
    // Query "commands", doc A has "command", doc B has "commands",
    // doc C is disjoint (must stay scoreless).
    let entries = vec![
        mk("run the deploy command now"),
        mk("run the deploy commands now"),
        mk("unrelated weather forecast"),
    ];
    let ranked = bm25_rank(&entries, "deploy commands", 10);
    let score = |idx: usize| ranked.iter().find(|(i, _)| *i == idx).map(|(_, s)| *s);
    let (a, b) = (score(0), score(1));
    assert!(a.is_some() && b.is_some(), "both tied docs must score");
    assert!(
        (a.unwrap() - b.unwrap()).abs() < 1e-6,
        "tied plurality pair must score equal: {a:?} vs {b:?}"
    );
    assert!(score(2).is_none(), "disjoint doc must stay scoreless");
}

#[test]
fn bm25_plural_fold_reaches_no_new_doc() {
    // c2b narrowing (reachability-preserving support gate): a doc the
    // unfolded query never reaches must stay scoreless even when folding
    // would conjure a match (`codes`->`code` vs query `code`). This is the
    // observed 04 M-006 harm shape: the distractor gains a folded-only
    // match for an already-matching singular query term.
    let mk = |content: &str| MemoryEntry::new(MemoryCategory::Fact, content);
    let entries = vec![
        mk("c2bnarrow locker log arranged visitor locker bank code for the new arrival"),
        mk("c2bnarrow door codes floor staging passcode rotates monthly"),
    ];
    // Control: without folding the distractor is unreachable (sanity that
    // this test exercises the conjure class, not a pristine match).
    let ranked = bm25_rank(&entries, "what visitor locker code did buddy arrange", 10);
    let ids: Vec<usize> = ranked.iter().map(|(i, _)| *i).collect();
    assert_eq!(
        ids,
        vec![0],
        "unsupported distractor must stay scoreless under the narrowed rule; got {ids:?}"
    );
}

#[test]
fn c2a_bm25f_tag_boost_direction() {
    // BM25F-lite: a tagged clone of a content-tied pair must outrank the
    // untagged twin at boost=2 (tag match adds boost*tf_t/B_t BEFORE
    // saturation). Pool: the tied pair + one distractor sharing no terms.
    let plain = MemoryEntry::new(
        MemoryCategory::Fact,
        "The deploy freeze lifts Monday morning after review",
    );
    let mut tagged = MemoryEntry::new(
        MemoryCategory::Fact,
        "The deploy freeze lifts Monday morning after review",
    );
    tagged = tagged.with_tags(vec!["deploy".to_string()]);
    let distractor = MemoryEntry::new(
        MemoryCategory::Fact,
        "Unrelated note about coffee brewing temperatures",
    );
    let entries = vec![plain.clone(), tagged.clone(), distractor];
    let ranked = bm25_rank(&entries, "deploy freeze schedule", 3);
    assert_eq!(ranked.len(), 2, "only the tied pair overlaps the query");
    assert_eq!(
        ranked[0].0, 1,
        "tagged clone must outrank the untagged twin"
    );
    assert_eq!(ranked[1].0, 0);
    assert!(
        ranked[0].1 > ranked[1].1,
        "tag boost must raise the score: {} vs {}",
        ranked[0].1,
        ranked[1].1
    );
}

#[test]
fn bm25_plural_fold_keeps_supported_plural_rescue() {
    // c2b narrowing must NOT break the rescue direction: a doc that
    // already engages the query keeps its folded plural match. Shape of
    // the dev-W1 items (e.g. R-001: query `start`, gold has `starts` plus
    // other shared terms): the `starts`->`start` fold must still count.
    let mk = |content: &str| MemoryEntry::new(MemoryCategory::Fact, content);
    let entries = vec![
        mk("c2bnarrow friday deploy freeze starts evening pager stays"),
        mk("c2bnarrow unrelated weather forecast"),
    ];
    let ranked = bm25_rank(&entries, "when does the deploy freeze start", 10);
    let score0 = ranked.iter().find(|(i, _)| *i == 0).map(|(_, s)| *s);
    assert!(
        score0.is_some(),
        "supported doc must score under the narrowed rule"
    );
    assert!(
        ranked.iter().all(|(i, _)| *i == 0),
        "disjoint doc must stay scoreless"
    );
    // The folded plural term must contribute: `starts` folds to `start`
    // which matches the query term, so doc 0 must beat a doc sharing only
    // the uninflected terms.
    let entries2 = vec![
        mk("c2bnarrow friday deploy freeze starts evening pager stays"),
        mk("c2bnarrow friday deploy freeze evening pager stays"),
    ];
    let ranked2 = bm25_rank(&entries2, "when does the deploy freeze start", 10);
    let s = |idx: usize| ranked2.iter().find(|(i, _)| *i == idx).map(|(_, s)| *s);
    assert!(
        s(0).unwrap() > s(1).unwrap(),
        "folded plural match must add score on a supported doc: {:?} vs {:?}",
        s(0),
        s(1)
    );
}

#[test]
fn bm25_bigram_shared_collocation_beats_separated_unigrams() {
    // c2c mechanism: a doc sharing an adjacent query bigram must outscore
    // a doc sharing only the same separated unigrams. Same word multiset
    // in both docs (identical unigram BM25), so the gap is pure bonus.
    let mk = |content: &str| MemoryEntry::new(MemoryCategory::Fact, content);
    let entries = vec![
        mk("c2c deploy freeze alpha beta"),
        mk("c2c deploy alpha beta freeze"),
        mk("c2c unrelated weather forecast"),
    ];
    let ranked = bm25_rank(&entries, "deploy freeze", 10);
    let score =
        |idx: usize| ranked.iter().find(|(i, _)| *i == idx).map(|(_, s)| *s);
    let (adj, sep) = (score(0), score(1));
    assert!(adj.is_some() && sep.is_some(), "both docs share unigrams");
    assert!(
        adj.unwrap() > sep.unwrap(),
        "adjacent-shared-bigram doc must beat separated-unigrams doc: {adj:?} vs {sep:?}"
    );
    assert!(score(2).is_none(), "disjoint doc must stay scoreless");
}

#[test]
fn bm25_bigram_stopword_bonus_less_than_content_bonus() {
    // c2c self-discount (§4.1): on matched adjacent/separated pairs with
    // identical unigram multisets, the per-pair gap is exactly the bigram
    // bonus; the stopword-carrying pair's gap must be smaller than the
    // content pair's gap. Background docs carry "the" so its IDF discounts.
    let mk = |content: &str| MemoryEntry::new(MemoryCategory::Fact, content);
    let bgs = [mk("c2c the weather today"), mk("c2c the quick fox")];
    let gap = |query: &str, adj: &str, sep: &str| {
        let entries = vec![mk(adj), mk(sep), bgs[0].clone(), bgs[1].clone()];
        let ranked = bm25_rank(&entries, query, 10);
        let score =
            |idx: usize| ranked.iter().find(|(i, _)| *i == idx).map(|(_, s)| *s);
        score(0).unwrap() - score(1).unwrap()
    };
    let stop_gap = gap("the staging", "c2c the staging xray", "c2c the xray staging");
    let content_gap = gap(
        "staging database",
        "c2c staging database xray",
        "c2c staging xray database",
    );
    assert!(stop_gap > 0.0, "stopword bigram bonus must be positive");
    assert!(
        content_gap > stop_gap,
        "content-bigram bonus must exceed stopword-bigram bonus: {content_gap} vs {stop_gap}"
    );
}

#[test]
fn c2a_bm25f_tagless_pool_parity() {
    // Content-only bound: on a tagless pool the BM25F-lite ranking must
    // reproduce the flat-join BM25 order (additive form cannot veto a
    // content match; near-identical scores, same order).
    let docs = vec![
        "The production deploy freeze starts Friday evening",
        "Staging database password rotation happens quarterly",
        "The deploy freeze lifts Monday morning after review",
        "Unrelated note about bicycle maintenance schedules",
    ];
    let entries: Vec<MemoryEntry> = docs
        .iter()
        .map(|d| MemoryEntry::new(MemoryCategory::Fact, *d))
        .collect();
    let ranked = bm25_rank(&entries, "deploy freeze schedule", 4);
    assert!(!ranked.is_empty(), "content matches must survive");
    // The two deploy-freeze docs outrank the rest, in content order.
    let top_ids: Vec<usize> = ranked.iter().map(|(i, _)| *i).collect();
    assert!(top_ids.contains(&0) && top_ids.contains(&2));
    assert_eq!(top_ids[0], 0, "exact deploy-freeze doc ranks first");
    // Scores are finite and strictly positive for all returned docs.
    for (i, s) in &ranked {
        assert!(s.is_finite() && *s > 0.0, "doc {i} score {s}");
    }
}

#[test]
fn c2a_guard_stopword_tag_keeps_no_alive_without_content_match() {
    // c2a-guard mechanism, CORRECTED shape (04-rescore M-006 forensics):
    // the keep-alive distractor matches the stopword `the` in CONTENT
    // (short content => sub-unity FieldDoc norm AMPLIFIES its tf above
    // the longer gold's). Two drop-rule variants were tried and REJECTED
    // (both measured on the real corpus): per-term stopword veto = inert,
    // per-doc stopword-only veto = backfires (drops the boundary gold
    // itself). The guard is therefore a NORM FLOOR: a doc with no
    // unfolded non-stopword overlap gets b_c floored at 1.0 (no
    // sub-unity amplification), so the short stopword-only distractor
    // can no longer outrank the gold on length grounds. The distractor
    // still scores (reachability preserved) but below the gold.
    let mut gold = MemoryEntry::new(MemoryCategory::Fact, "c2aguard buddy morning arrival");
    gold = gold.with_tags(vec!["buddy".to_string()]);
    // Stopword-content distractor: short content (norm-amplified tf, the
    // FieldDoc failure shape) + query-disjoint tags.
    let mut stop_only = MemoryEntry::new(
        MemoryCategory::Fact,
        "c2aguard replace the hallway",
    );
    stop_only = stop_only.with_tags(vec!["c2aguard".to_string()]);
    // Supported (shares unfolded `buddy`), so c2b folding applies; models
    // the legitimate `buddies->buddy`-class lift that compresses the
    // gold's IDF without conjuring reach.
    let folded = MemoryEntry::new(MemoryCategory::Fact, "c2aguard buddy door codes floor");
    // Background docs: reproduce the real-corpus IDF structure (`the`
    // ubiquitous => low IDF; `buddy` rare => high IDF). Without them the
    // tiny pool inverts the IDFs (`the` rare) and the test proves nothing
    // about the corpus failure shape. Background docs share no query
    // content terms (they only carry `the`, like most real-corpus docs).
    let bg = |content: &str| MemoryEntry::new(MemoryCategory::Fact, content);
    let entries = vec![
        gold,
        stop_only,
        folded,
        bg("c2aguard the weather today"),
        bg("c2aguard the quick fox"),
        bg("c2aguard under the table"),
        bg("c2aguard over the fence"),
    ];
    let ranked = bm25_rank(&entries, "what did the buddy arrange", 10);
    let ids: Vec<usize> = ranked.iter().map(|(i, _)| *i).collect();
    assert_eq!(
        ids[0], 0,
        "guard must keep gold first (norm floor caps the short stopword-only distractor); got {ranked:?}"
    );
    assert!(
        ids.contains(&1),
        "stopword-only distractor stays reachable (floor lowers, never drops); got {ranked:?}"
    );
    // Guard preserves c2a dev value: tagged gold keeps its boost over an
    // otherwise-identical untagged twin.
    let entries2 = vec![
        MemoryEntry::new(MemoryCategory::Fact, "c2aguard buddy morning arrival"),
        entries[0].clone(),
    ];
    let ranked2 = bm25_rank(&entries2, "what did the buddy arrange", 10);
    let s = |idx: usize| ranked2.iter().find(|(i, _)| *i == idx).map(|(_, s)| *s);
    assert_eq!(
        ranked2.len(),
        2,
        "both golds overlap the query; got {ranked2:?}"
    );
    assert!(
        s(1).unwrap() > s(0).unwrap(),
        "tagged gold must keep its boost over the untagged twin: {:?} vs {:?}",
        s(1),
        s(0)
    );
}

#[test]
fn prefilter_shadow_snapshot_matches_tuple_accessor() {
    // Read-only check: the struct snapshot and the historical tuple helper
    // read the same four process-wide atomics in the same order.
    let snapshot = MemoryManager::prefilter_shadow_snapshot();
    let (queries, engaged, failopen, sampled) = MemoryManager::prefilter_shadow_stats();
    assert_eq!(snapshot.queries, queries);
    assert_eq!(snapshot.engaged, engaged);
    assert_eq!(snapshot.failopen, failopen);
    assert_eq!(snapshot.shadow_sampled, sampled);
}

#[test]
fn prefilter_shadow_snapshot_json_shape() {
    // Empty-stats JSON shape for the --json CLI surface: all four counter
    // keys present with the documented names.
    let snapshot = PrefilterShadowSnapshot {
        queries: 0,
        engaged: 0,
        failopen: 0,
        shadow_sampled: 0,
    };
    let value = serde_json::to_value(&snapshot).expect("snapshot serializes");
    assert_eq!(value["queries"], 0);
    assert_eq!(value["engaged"], 0);
    assert_eq!(value["failopen"], 0);
    assert_eq!(value["shadow_sampled"], 0);
    assert!(snapshot.dropped_tail_summary().contains("0 queries"));
}

#[test]
fn prefilter_failopen_arms_cover_all_five_sites_in_order() {
    // Item 1: the arm enum maps 1:1 onto the five conflated FAILOPEN sites
    // (Metric 4: embed error, embed-over-budget, rank-over-budget,
    // empty-from-nonempty, post-stage over-budget).
    assert_eq!(PrefilterFailopenArm::ALL.len(), 5);
    for (i, arm) in PrefilterFailopenArm::ALL.iter().enumerate() {
        assert_eq!(arm.index(), i, "arm order must match atomic index");
    }
    let names: Vec<&str> = PrefilterFailopenArm::ALL.iter().map(|a| a.name()).collect();
    assert_eq!(
        names,
        [
            "embed_error",
            "embed_over_budget",
            "rank_over_budget",
            "empty_from_nonempty",
            "post_stage_over_budget"
        ]
    );
}

#[test]
fn prefilter_failopen_arm_counter_increments_matching_slot() {
    // Item 1 read site: noting arm N bumps slot N and only slot N.
    // Serialized: bumps are process-wide, exact deltas need isolation.
    let _shadow = PREFILTER_SHADOW_TEST_LOCK
        .lock()
        .expect("prefilter shadow test lock poisoned");
    for arm in PrefilterFailopenArm::ALL {
        let before = MemoryManager::prefilter_failopen_arms();
        MemoryManager::note_prefilter_failopen(arm);
        let after = MemoryManager::prefilter_failopen_arms();
        for (i, (b, a)) in before.iter().zip(after.iter()).enumerate() {
            if i == arm.index() {
                assert_eq!(*a, b + 1, "arm {} must bump its own slot", arm.name());
            } else {
                assert_eq!(*a, *b, "arm {} must not touch slot {i}", arm.name());
            }
        }
    }
}

#[test]
fn prefilter_rank_guarded_empty_from_nonempty_bumps_arm() {
    // Item 1 behavioral: a nonempty input with zero rank overlap fails open
    // via the empty-from-nonempty arm (no embed backend needed — the rank
    // guard takes the query embedding as a parameter).
    let _guard = crate::storage::lock_test_env();
    let _shadow = PREFILTER_SHADOW_TEST_LOCK
        .lock()
        .expect("prefilter shadow test lock poisoned");
    let before = MemoryManager::prefilter_failopen_arms();
    let entries = vec![
        MemoryEntry::new(MemoryCategory::Fact, "alpha beta gamma delta"),
        MemoryEntry::new(MemoryCategory::Fact, "epsilon zeta eta theta"),
    ];
    let mut slot = Some(entries);
    let staged = MemoryManager::prefilter_rank_guarded(
        &mut slot,
        "zzqx jumbled vapor qwerty",
        Instant::now(),
        Duration::from_secs(60),
        24,
        &[],
    );
    assert!(staged.is_none(), "zero-overlap input must fail open");
    assert_eq!(slot.map(|v| v.len()), Some(2), "input restored untouched");
    let after = MemoryManager::prefilter_failopen_arms();
    assert_eq!(
        after[PrefilterFailopenArm::EmptyFromNonempty.index()],
        before[PrefilterFailopenArm::EmptyFromNonempty.index()] + 1,
        "empty arm must bump exactly once"
    );
    for (i, (b, a)) in before.iter().zip(after.iter()).enumerate() {
        if i != PrefilterFailopenArm::EmptyFromNonempty.index() {
            assert_eq!(a, b, "other arm slot {i} must not move");
        }
    }
}

#[test]
fn prefilter_rank_guarded_embed_over_budget_bumps_arm() {
    // Item 1 behavioral: a stage_start already past the budget trips the
    // embed-over-budget arm deterministically (no sleep, no flake).
    let _guard = crate::storage::lock_test_env();
    let _shadow = PREFILTER_SHADOW_TEST_LOCK
        .lock()
        .expect("prefilter shadow test lock poisoned");
    let before = MemoryManager::prefilter_failopen_arms();
    let entries = vec![
        MemoryEntry::new(MemoryCategory::Fact, "alpha beta gamma delta"),
        MemoryEntry::new(MemoryCategory::Fact, "epsilon zeta eta theta"),
    ];
    let mut slot = Some(entries);
    let staged = MemoryManager::prefilter_rank_guarded(
        &mut slot,
        "alpha beta",
        Instant::now() - Duration::from_secs(3600),
        Duration::from_millis(500),
        24,
        &[],
    );
    assert!(staged.is_none(), "stale stage must fail open");
    assert_eq!(slot.map(|v| v.len()), Some(2), "input restored untouched");
    let after = MemoryManager::prefilter_failopen_arms();
    assert_eq!(
        after[PrefilterFailopenArm::EmbedOverBudget.index()],
        before[PrefilterFailopenArm::EmbedOverBudget.index()] + 1,
        "embed-over-budget arm must bump exactly once"
    );
}

#[test]
fn prefilter_rank_guarded_success_records_rank_elapsed() {
    // Item 3 behavioral: a ranked (nonempty) pass records one rank sample
    // even when unsampled (no tail pass, dropped empty).
    let _guard = crate::storage::lock_test_env();
    let _shadow = PREFILTER_SHADOW_TEST_LOCK
        .lock()
        .expect("prefilter shadow test lock poisoned");
    let before = MemoryManager::prefilter_stage_stats();
    let entries = vec![
        MemoryEntry::new(MemoryCategory::Fact, "alpha beta gamma delta"),
        MemoryEntry::new(MemoryCategory::Fact, "alpha beta epsilon zeta"),
        MemoryEntry::new(MemoryCategory::Fact, "alpha eta theta iota"),
    ];
    let mut slot = Some(entries);
    let staged = MemoryManager::prefilter_rank_guarded(
        &mut slot,
        "alpha beta",
        Instant::now(),
        Duration::from_secs(60),
        24,
        &[],
    )
    .expect("overlapping input must rank");
    assert!(!staged.kept.is_empty(), "kept set must be nonempty");
    let after = MemoryManager::prefilter_stage_stats();
    assert_eq!(after.rank_count, before.rank_count + 1);
    assert!(after.rank_sum_us >= before.rank_sum_us);
    assert!(after.rank_max_us >= before.rank_max_us);
    assert_eq!(after.rank_sum_sq_us, before.rank_sum_sq_us + overshoot_sq_guard(before, after));
}

#[test]
fn prefilter_stage_stats_mean_math() {
    // Item 3 read site: means are sum/count, 0.0 with no samples.
    let empty = PrefilterStageStats::default();
    assert_eq!(empty.embed_mean_us(), 0.0);
    assert_eq!(empty.rank_mean_us(), 0.0);
    let stats = PrefilterStageStats {
        embed_count: 4,
        embed_sum_us: 100,
        rank_count: 2,
        rank_sum_us: 50,
        ..PrefilterStageStats::default()
    };
    assert_eq!(stats.embed_mean_us(), 25.0);
    assert_eq!(stats.rank_mean_us(), 25.0);
    let value = serde_json::to_value(&stats).expect("stage stats serialize");
    assert_eq!(value["embed_count"], 4);
    assert_eq!(value["rank_sum_us"], 50);
}

/// Helper: expected `sum_sq` delta for the single new rank sample is the
/// square of the single new `sum` delta (exactly one sample was added).
fn overshoot_sq_guard(before: PrefilterStageStats, after: PrefilterStageStats) -> u64 {
    let delta = after.rank_sum_us - before.rank_sum_us;
    delta.saturating_mul(delta)
}

#[test]
fn prefilter_engaged_sampled_reader_tracks_joint_counter() {
    // Item 2 read site: the joint counter starts coherent (<= both
    // marginals) and the reader reflects increments.
    let engaged = MemoryManager::prefilter_shadow_snapshot().engaged;
    let sampled = MemoryManager::prefilter_shadow_snapshot().shadow_sampled;
    let joint = MemoryManager::prefilter_engaged_sampled();
    assert!(
        joint <= engaged && joint <= sampled,
        "joint {joint} must be bounded by engaged {engaged} and sampled {sampled}"
    );
}

#[test]
fn prefilter_tail_detail_hashes_query_and_bounds_tail() {
    // Item 4 shape: query hash is deterministic hex, raw text never
    // stored, tail IDs bounded with totals + truncation flag.
    let kept = vec![MemoryEntry::new(MemoryCategory::Fact, "alpha beta")];
    let dropped: Vec<MemoryEntry> = (0..3)
        .map(|i| MemoryEntry::new(MemoryCategory::Fact, format!("tail entry {i}")))
        .collect();
    let a = MemoryManager::prefilter_tail_detail("my secret query", &kept, &dropped, 96, 0.01);
    let b = MemoryManager::prefilter_tail_detail("my secret query", &kept, &dropped, 96, 0.01);
    assert_eq!(a.query_hash, b.query_hash, "hash must be deterministic");
    assert_eq!(a.query_hash.len(), 16, "hash must be 16 hex chars");
    assert!(
        a.query_hash.chars().all(|c| c.is_ascii_hexdigit()),
        "hash must be hex"
    );
    let c = MemoryManager::prefilter_tail_detail("other query", &kept, &dropped, 96, 0.01);
    assert_ne!(a.query_hash, c.query_hash, "distinct queries hash distinctly");
    let serialized = serde_json::to_string(&a).expect("detail serializes");
    assert!(
        !serialized.contains("my secret query"),
        "raw query text must never appear in the log line"
    );
    assert_eq!(a.kept_total, 1);
    assert_eq!(a.kept_ids.len(), 1);
    assert_eq!(a.tail_total, 3);
    assert_eq!(a.tail_ids.len(), 3);
    assert!(!a.tail_truncated);
    assert_eq!(a.top_k, 96);
    assert_eq!(a.rate, 0.01);
    assert_eq!(a.split_tag, "live");
}

#[test]
fn prefilter_tail_detail_truncates_huge_tails() {
    // Item 4 bound: a corpus-scale tail logs at most
    // PREFILTER_TAIL_LOG_MAX_IDS ids with the full total preserved.
    let kept = vec![MemoryEntry::new(MemoryCategory::Fact, "alpha beta")];
    let dropped: Vec<MemoryEntry> = (0..(PREFILTER_TAIL_LOG_MAX_IDS + 7))
        .map(|i| MemoryEntry::new(MemoryCategory::Fact, format!("tail entry {i}")))
        .collect();
    let detail = MemoryManager::prefilter_tail_detail("big query", &kept, &dropped, 96, 0.01);
    assert_eq!(detail.tail_total, PREFILTER_TAIL_LOG_MAX_IDS + 7);
    assert_eq!(detail.tail_ids.len(), PREFILTER_TAIL_LOG_MAX_IDS);
    assert!(detail.tail_truncated);
}

#[test]
fn prefilter_rank_guarded_leaves_slot_none_and_returns_kept() {
    // Simplify-first pin: the rank guard no longer does the store-take
    // dance (`*entries_opt = Some(kept); take()`). On success the slot is
    // None and the kept set flows out in the staged struct; the caller
    // (`prefilter_for_jev_take`) puts it back. Fail-open arms still
    // restore the FULL input (slot is Some) — drop/fail-open semantics
    // unchanged.
    let _guard = crate::storage::lock_test_env();
    let _shadow = PREFILTER_SHADOW_TEST_LOCK
        .lock()
        .expect("prefilter shadow test lock poisoned");
    let entries = vec![
        MemoryEntry::new(MemoryCategory::Fact, "alpha beta gamma delta"),
        MemoryEntry::new(MemoryCategory::Fact, "alpha beta epsilon zeta"),
        MemoryEntry::new(MemoryCategory::Fact, "alpha eta theta iota"),
    ];
    let mut slot = Some(entries);
    let staged = MemoryManager::prefilter_rank_guarded(
        &mut slot,
        "alpha beta",
        Instant::now(),
        Duration::from_secs(60),
        24,
        &[],
    )
    .expect("overlapping input must rank");
    assert!(slot.is_none(), "slot stays None: caller restores kept");
    assert!(!staged.kept.is_empty(), "kept flows out in staged struct");
    assert!(
        staged.kept.len() <= 24,
        "kept respects top_k (test top_k=24)"
    );
}

#[test]
fn prefilter_for_jev_take_disengaged_leaves_slot_some() {
    // Coordinator pin: the take-level contract is "always leaves a set in
    // entries_opt (never None on return)". The disengaged path (stage off
    // by default) restores the input untouched and returns None.
    let _guard = crate::storage::lock_test_env();
    let _shadow = PREFILTER_SHADOW_TEST_LOCK
        .lock()
        .expect("prefilter shadow test lock poisoned");
    let entries = vec![
        MemoryEntry::new(MemoryCategory::Fact, "alpha beta gamma delta"),
        MemoryEntry::new(MemoryCategory::Fact, "epsilon zeta eta theta"),
    ];
    let mut slot = Some(entries);
    let tail = MemoryManager::prefilter_for_jev_take(&mut slot, "alpha beta");
    assert!(tail.is_none(), "disengaged stage returns no tail");
    let back = slot.expect("entries_opt must be Some after the call");
    assert_eq!(back.len(), 2, "disengaged input flows through untouched");
}

#[test]
fn prefilter_wilson_upper_95_bounds_rates() {
    // Gate math: vacuous with no samples, above the point estimate with
    // samples, monotone in hits, sane at the 0/100 textbook corner.
    assert_eq!(MemoryManager::wilson_upper_95(0, 0), 1.0);
    let zero_of_100 = MemoryManager::wilson_upper_95(0, 100);
    assert!(
        (0.03..0.05).contains(&zero_of_100),
        "0/100 upper must be ~0.037, got {zero_of_100}"
    );
    let fifty = MemoryManager::wilson_upper_95(50, 100);
    assert!(
        fifty > 0.5 && fifty < 0.6,
        "50/100 upper must bracket 0.5 from above, got {fifty}"
    );
    let full = MemoryManager::wilson_upper_95(100, 100);
    assert!(
        full > 0.96 && full <= 1.0,
        "100/100 upper must be ~1.0, got {full}"
    );
    assert!(
        MemoryManager::wilson_upper_95(1, 10) < MemoryManager::wilson_upper_95(9, 10),
        "upper must grow with hits at fixed n"
    );
}

#[test]
fn prefilter_tail_hit_snapshot_rates_and_bounds() {
    // Aggregation seam: `note_shadow_tail_rejudged` bumps rejudged always
    // and hits only on tail_accepts > 0; the snapshot reports the
    // query-level rate with the Wilson upper bound beside the joint
    // (ENGAGED_SAMPLED) denominator cross-check.
    let _guard = crate::storage::lock_test_env();
    let _shadow = PREFILTER_SHADOW_TEST_LOCK
        .lock()
        .expect("prefilter shadow test lock poisoned");
    let before = MemoryManager::prefilter_tail_hit_snapshot();
    MemoryManager::note_shadow_tail_rejudged("hash-hit", "live", 3);
    MemoryManager::note_shadow_tail_rejudged("hash-clean", "live", 0);
    let after = MemoryManager::prefilter_tail_hit_snapshot();
    assert_eq!(after.rejudged, before.rejudged + 2);
    assert_eq!(after.tail_hits, before.tail_hits + 1);
    assert!(
        after.tail_hits <= after.rejudged,
        "hits can never exceed rejudged (hit implies a completed re-judge)"
    );
    let expected = after.tail_hits as f64 / after.rejudged as f64;
    assert!(
        (after.tail_hit - expected).abs() < 1e-12,
        "tail_hit must be hits/rejudged"
    );
    assert!(
        after.tail_hit_upper_95 >= after.tail_hit,
        "Wilson upper must bound the point estimate"
    );
    assert_eq!(
        after.tail_hit_upper_95,
        MemoryManager::wilson_upper_95(after.tail_hits, after.rejudged),
        "snapshot upper must match the pure Wilson helper"
    );
    let value = serde_json::to_value(&after).expect("tail-hit snapshot serializes");
    assert!(value.get("tail_hit").is_some());
    assert!(value.get("tail_hit_upper_95").is_some());
    // Split lives per log line (tail + rejudge), not in the aggregate:
    // the gate filters offline on `split_tag` before counting.
    assert!(value.get("split_tag").is_none());
}
