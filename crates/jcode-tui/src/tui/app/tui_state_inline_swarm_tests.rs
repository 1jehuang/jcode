use super::{filter_inline_swarm_subtree, has_inline_swarm_descendants};
use crate::protocol::SwarmMemberStatus;
use crate::tui::TuiState as _;

#[test]
fn boolean_matches_filter_exhaustive_small_graphs() {
    // All ordered lists through four records, including duplicate IDs,
    // missing parents, self loops, cycles, and a parent with no record.
    let ids = ["me", "a", "b"];
    let parents = [None, Some("me"), Some("a"), Some("b"), Some("missing")];
    for len in 0..=4 {
        for mut encoding in 0..15usize.pow(len) {
            let mut members = Vec::new();
            for _ in 0..len {
                let edge = encoding % 15;
                encoding /= 15;
                members.push(member(ids[edge / 5], parents[edge % 5]));
            }
            for self_id in ["me", "a", "missing"] {
                assert_eq!(
                    has_inline_swarm_descendants(&members, self_id),
                    !filter_inline_swarm_subtree(&members, self_id).is_empty(),
                    "len={len}, self={self_id}, members={members:?}"
                );
            }
        }
    }
}

#[test]
fn boolean_app_modes_match_owned_members_and_gallery() {
    let mut app = crate::tui::app::App::new_for_remote(None);
    for debug in [false, true] {
        for enabled in [false, true] {
            for remote in [false, true] {
                for identity in [None, Some("me")] {
                    for members in [
                        vec![],
                        vec![member("me", Some("me"))],
                        vec![member("child", Some("me"))],
                        vec![member("other", Some("elsewhere"))],
                    ] {
                        app.debug_force_inline_gallery = debug;
                        app.swarm_enabled = enabled;
                        app.is_remote = remote;
                        app.remote_session_id = identity.map(str::to_owned);
                        app.session.id = "me".to_owned();
                        app.remote_swarm_members = members;
                        app.remote_swarm_members_received_at = Some(std::time::Instant::now());
                        let expected = !app.inline_swarm_members().is_empty();
                        assert_eq!(app.has_inline_swarm_members(), expected);
                        let active = if debug {
                            expected
                        } else {
                            enabled
                                && matches!(
                                    crate::config::config().agents.swarm_spawn_mode,
                                    crate::config::SwarmSpawnMode::Inline
                                )
                                && expected
                        };
                        assert_eq!(app.inline_swarm_gallery_active(), active);
                    }
                }
            }
        }
    }
}

#[test]
fn boolean_does_not_replace_or_mutate_elapsed_ticking() {
    let mut app = crate::tui::app::App::new_for_remote(None);
    app.remote_session_id = Some("me".to_owned());
    app.swarm_enabled = true;
    let mut child = member("child", Some("me"));
    child.runtime.elapsed_secs = Some(7);
    let mut done = member("done", Some("me"));
    done.status = "completed".to_owned();
    done.runtime.elapsed_secs = Some(11);
    app.remote_swarm_members = vec![child, done];
    app.remote_swarm_members_received_at =
        Some(std::time::Instant::now() - std::time::Duration::from_secs(3));
    assert!(app.has_inline_swarm_members());
    let rendered = app.inline_swarm_members();
    assert!(rendered[0].runtime.elapsed_secs.unwrap() >= 10);
    assert_eq!(rendered[1].runtime.elapsed_secs, Some(11));
    assert_eq!(app.remote_swarm_members[0].runtime.elapsed_secs, Some(7));
    assert_eq!(app.remote_swarm_members[1].runtime.elapsed_secs, Some(11));
}

#[test]
#[ignore = "paired microbenchmark, run explicitly with --profile selfdev --nocapture"]
fn boolean_paired_benchmark() {
    use std::{hint::black_box, time::Instant};
    for size in [0, 8, 64, 256, 1024] {
        for position in ["first", "last", "absent"] {
            let members: Vec<_> = (0..size)
                .map(|i| {
                    let child =
                        (position == "first" && i == 0) || (position == "last" && i + 1 == size);
                    member(
                        &format!("worker-{i}"),
                        Some(if child { "me" } else { "other" }),
                    )
                })
                .collect();
            let iterations = 2000;
            // Alternate order across rounds to reduce ordering bias.
            let mut old = std::time::Duration::ZERO;
            let mut new = std::time::Duration::ZERO;
            for round in 0..6 {
                for optimized in if round % 2 == 0 {
                    [false, true]
                } else {
                    [true, false]
                } {
                    let start = Instant::now();
                    for _ in 0..iterations {
                        black_box(if optimized {
                            has_inline_swarm_descendants(black_box(&members), black_box("me"))
                        } else {
                            !filter_inline_swarm_subtree(black_box(&members), black_box("me"))
                                .is_empty()
                        });
                    }
                    if optimized {
                        new += start.elapsed();
                    } else {
                        old += start.elapsed();
                    }
                }
            }
            eprintln!(
                "swarm boolean n={size} child={position} old_ns={:.1} new_ns={:.1}",
                old.as_nanos() as f64 / (iterations * 6) as f64,
                new.as_nanos() as f64 / (iterations * 6) as f64
            );
        }
    }
}

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
