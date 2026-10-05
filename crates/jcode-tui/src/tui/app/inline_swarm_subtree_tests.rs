//! Tests for `#[path]`-attributed module `inline_swarm_subtree_tests` of `tui_state.rs`.
//!
//! Extracted from the parent file: an inline `#[cfg(test)] mod` counts toward
//! the code-size budget, which only exempts `*_tests.rs` files.

use super::filter_inline_swarm_subtree;
use crate::protocol::SwarmMemberStatus;

fn member(id: &str, parent: Option<&str>) -> SwarmMemberStatus {
    SwarmMemberStatus {
        session_id: id.to_string(),
        friendly_name: Some(id.to_string()),
        status: "running".to_string(),
        detail: None,
        task_label: None,
        role: None,
        is_headless: Some(true),
        live_attachments: None,
        status_age_secs: Some(1),
        output_tail: None,
        report_back_to_session_id: parent.map(str::to_string),
        todo_progress: None,
        todo_items: Vec::new(),
        runtime: crate::protocol::SwarmMemberRuntime::default(),
    }
}

fn ids(members: Vec<SwarmMemberStatus>) -> Vec<String> {
    let mut v: Vec<String> = members.into_iter().map(|m| m.session_id).collect();
    v.sort();
    v
}

#[test]
fn includes_direct_children_but_not_self() {
    let members = vec![
        member("me", None),
        member("child_a", Some("me")),
        member("child_b", Some("me")),
        member("stranger", None),
    ];
    // The viewing session ("me") is excluded; only its spawned children show.
    assert_eq!(
        ids(filter_inline_swarm_subtree(&members, "me")),
        vec!["child_a", "child_b"]
    );
}

#[test]
fn includes_transitive_descendants() {
    let members = vec![
        member("me", None),
        member("child", Some("me")),
        member("grandchild", Some("child")),
    ];
    assert_eq!(
        ids(filter_inline_swarm_subtree(&members, "me")),
        vec!["child", "grandchild"]
    );
}

#[test]
fn excludes_siblings_and_unrelated_sessions() {
    // Two coordinators sharing one swarm. Each should only see its own kids.
    let members = vec![
        member("coord_a", None),
        member("a_child", Some("coord_a")),
        member("coord_b", None),
        member("b_child", Some("coord_b")),
    ];
    assert_eq!(
        ids(filter_inline_swarm_subtree(&members, "coord_a")),
        vec!["a_child"]
    );
    assert_eq!(
        ids(filter_inline_swarm_subtree(&members, "coord_b")),
        vec!["b_child"]
    );
}

#[test]
fn session_with_no_children_shows_nothing() {
    // A session that spawned no one (even if it is itself a swarm member)
    // produces an empty list so the strip is hidden entirely.
    let members = vec![
        member("me", None),
        member("stranger", None),
        member("other", None),
    ];
    assert!(filter_inline_swarm_subtree(&members, "me").is_empty());
}

#[test]
fn cycle_is_guarded() {
    // Pathological parent cycle must not loop forever.
    let members = vec![
        member("a", Some("b")),
        member("b", Some("a")),
        member("me", None),
        member("child", Some("me")),
    ];
    assert_eq!(
        ids(filter_inline_swarm_subtree(&members, "me")),
        vec!["child"]
    );
}
