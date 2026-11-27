use super::*;
use std::sync::Arc;

fn fixture(count: usize) -> String {
    serde_json::json!({"todos": (0..count).map(|i| serde_json::json!({
        "id": format!("task-{i}"), "content": format!("Task {i} 恢复"),
        "status": "pending", "priority": "high",
        "requires_user_auth": {"reason": "Approve rollout"}
    })).collect::<Vec<_>>()})
    .to_string()
}

#[test]
fn todo_payload_cache_reuses_exact_content_and_replaces_changed_payload() {
    let mut cache = TodoPayloadCache::default();
    let content = fixture(2);
    let first = cache.get(&content).unwrap();
    let same_content = content.clone();
    assert!(Arc::ptr_eq(&first, &cache.get(&same_content).unwrap()));
    let changed = content.replace("Task 0", "Task X");
    let next = cache.get(&changed).unwrap();
    assert!(!Arc::ptr_eq(&first, &next));
    assert_eq!(next.todos()[0].content, "Task X 恢复");
    assert!(!Arc::ptr_eq(&first, &cache.get(&content).unwrap()));
    assert!(cache.get("invalid").is_none());
    assert!(cache.entry.as_ref().unwrap().1.is_none());
    assert!(cache.get("invalid").is_none());
    assert_eq!(cache.get("[]").unwrap().todos().len(), 0);
}

#[test]
fn todo_payload_cache_does_not_retain_oversized_payloads() {
    let mut cache = TodoPayloadCache::default();
    cache.get("[]").unwrap();
    let oversized = format!(
        "{{\"todos\":[],\"extra\":\"{}\"}}",
        "x".repeat(MAX_CACHED_TODO_PAYLOAD_BYTES)
    );
    assert!(cache.get(&oversized).is_some());
    assert!(cache.entry.is_none());
    assert!(
        cache
            .get(&"x".repeat(MAX_CACHED_TODO_PAYLOAD_BYTES + 1))
            .is_none()
    );
    assert!(cache.entry.is_none());
}

#[test]
fn todo_payload_cache_preserves_width_rendering_and_resolution() {
    let content = fixture(1);
    let parsed = cached_todo_payload(&content).unwrap();
    let wide = todo_blocker_footer(Some(&content), 120);
    assert_eq!(wide.len(), 1);
    for width in [0, 1, 8, 24, 80] {
        let narrow = todo_blocker_footer(Some(&content), width);
        assert_eq!(narrow.len(), 1);
        assert!(narrow[0].width() <= usize::from(width));
        if width > 0 {
            assert_eq!(narrow[0].style.fg, Some(todo_warning_color()));
        }
    }
    assert!(Arc::ptr_eq(
        &parsed,
        &cached_todo_payload(&content).unwrap()
    ));
    assert_eq!(wide, todo_blocker_footer(Some(&content), 120));
    let resolved = content.replace("pending", "completed");
    assert!(todo_blocker_footer(Some(&resolved), 120).is_empty());
}

#[test]
fn todo_payload_cache_preserves_legacy_notes_and_resolved_content() {
    let content = serde_json::json!([{
        "id": "task", "content": "Device", "status": "pending", "priority": "high",
        "requires_user_auth": {"reason": "Connect hardware"}
    }])
    .to_string();
    for _ in 0..2 {
        assert!(
            todo_blocker_notes(Some(&content))[1]
                .to_string()
                .contains("Connect hardware")
        );
    }
    assert!(todo_blocker_notes(Some(&content.replace("pending", "completed"))).is_empty());
}

#[test]
#[ignore = "comparative hot-path benchmark, run with --ignored --nocapture"]
fn benchmark_todo_payload_cache_footer() {
    use std::hint::black_box;
    use std::time::Instant;
    for (count, blocked) in [(1, 1), (32, 1), (128, 1), (128, 128)] {
        let mut value: serde_json::Value = serde_json::from_str(&fixture(count)).unwrap();
        for todo in value["todos"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .skip(blocked)
        {
            todo.as_object_mut().unwrap().remove("requires_user_auth");
        }
        let content = value.to_string();
        let iterations = 1000;
        let parsed: TodoCardPayload = serde_json::from_str(&content).unwrap();
        assert_eq!(
            render_todo_blocker_footer(&parsed, 80),
            todo_blocker_footer(Some(&content), 80)
        );
        let start = Instant::now();
        for _ in 0..iterations {
            let parsed: TodoCardPayload = serde_json::from_str(black_box(&content)).unwrap();
            black_box(render_todo_blocker_footer(&parsed, 80));
        }
        let uncached = start.elapsed();
        todo_blocker_footer(Some(&content), 80);
        let start = Instant::now();
        for _ in 0..iterations {
            black_box(todo_blocker_footer(Some(black_box(&content)), 80));
        }
        let cached = start.elapsed();
        eprintln!(
            "todo footer end-to-end count={count} blocked={blocked} bytes={} iterations={iterations} uncached={uncached:?} cached={cached:?} speedup={:.2}x",
            content.len(),
            uncached.as_secs_f64() / cached.as_secs_f64()
        );
    }
}
