//! Tests for the current-directory pinned group in the session picker: the
//! sessions whose working directory matches the folder `/resume` was opened
//! from float to the top of the All view under their own header, with
//! running sessions (live process) sorted above stopped ones inside every
//! section.

use super::*;
use crate::tui::session_picker::filter::short_dir_label;
use chrono::{Duration as ChronoDuration, Utc};

/// Test-local mirror of the pre-filtered-search entry point used by
/// `/sessions <query>`: the picker opens with the query set, search mode on,
/// and the list already filtered. Rebuilt here so these tests exercise the
/// pinned-group-under-search behavior without depending on the separate
/// session-picker-search branch.
fn start_search(picker: &mut SessionPicker, query: &str) {
    picker.search_query = query.to_string();
    picker.search_active = true;
    picker.rebuild_items();
}

#[test]
fn test_current_dir_sessions_pin_to_top_with_header() {
    let mut old_here = make_session("old_here", "old-here", false, SessionStatus::Closed);
    old_here.working_dir = Some("/home/jeremy/project".to_string());
    old_here.last_message_time = Utc::now() - ChronoDuration::hours(2);
    let mut new_here = make_session("new_here", "new-here", false, SessionStatus::Closed);
    new_here.working_dir = Some("/home/jeremy/project/".to_string());
    new_here.last_message_time = Utc::now() - ChronoDuration::minutes(1);
    let mut elsewhere = make_session("elsewhere", "elsewhere", false, SessionStatus::Closed);
    elsewhere.working_dir = Some("/home/jeremy/elsewhere".to_string());
    elsewhere.last_message_time = Utc::now() - ChronoDuration::minutes(30);

    let mut picker =
        SessionPicker::new(vec![elsewhere.clone(), old_here.clone(), new_here.clone()]);
    picker.set_current_dir(Some("/home/jeremy/project".to_string()));
    picker.rebuild_items();

    // The pinned group header is the first item, followed by the current-dir
    // sessions in newest-first order, then the rest.
    assert!(matches!(
        picker.items.first(),
        Some(PickerItem::CurrentDirHeader { label, session_count })
            if *label == "/home/jeremy/project" && *session_count == 2
    ));
    let visible_ids: Vec<&str> = picker
        .visible_session_iter()
        .map(|session| session.id.as_str())
        .collect();
    assert_eq!(visible_ids, vec!["new_here", "old_here", "elsewhere"]);

    // Every item maps to the same visible-session order, so the preview and
    // navigation agree with the pinned layout.
    let first_session_item = picker
        .item_to_session
        .iter()
        .position(|slot| slot.is_some())
        .expect("at least one selectable item");
    assert!(matches!(
        picker.items[first_session_item],
        PickerItem::Session
    ));
    assert_eq!(picker.list_state.selected(), Some(first_session_item));
}

#[test]
fn test_current_dir_group_absent_without_matches_or_current_dir() {
    // No current_dir set: identical to the classic flat list.
    let session = make_session("plain", "plain", false, SessionStatus::Closed);
    let mut picker = SessionPicker::new(vec![session]);
    picker.rebuild_items();
    assert_eq!(picker.items.len(), 1);
    assert!(matches!(picker.items[0], PickerItem::Session));

    // current_dir set but no session matches: no pinned group is added.
    let mut session = make_session("nomatch", "nomatch", false, SessionStatus::Closed);
    session.working_dir = Some("/home/jeremy/other".to_string());
    let mut picker = SessionPicker::new(vec![session]);
    picker.set_current_dir(Some("/home/jeremy/project".to_string()));
    picker.rebuild_items();
    assert_eq!(picker.items.len(), 1);
    assert!(matches!(picker.items[0], PickerItem::Session));
}

#[test]
fn test_current_dir_sessions_not_duplicated_in_saved_or_other_sections() {
    let mut saved_here = make_session("saved_here", "saved-here", false, SessionStatus::Closed);
    saved_here.working_dir = Some("/home/jeremy/project".to_string());
    saved_here.saved = true;
    let mut saved_elsewhere = make_session(
        "saved_elsewhere",
        "saved-elsewhere",
        false,
        SessionStatus::Closed,
    );
    saved_elsewhere.working_dir = Some("/home/jeremy/elsewhere".to_string());
    saved_elsewhere.saved = true;

    let mut picker = SessionPicker::new(vec![saved_here.clone(), saved_elsewhere.clone()]);
    picker.set_current_dir(Some("/home/jeremy/project".to_string()));
    picker.rebuild_items();

    // The saved session from the current dir stays in the pinned group (with
    // its pin badge on the row) and does not also appear under Saved.
    let visible_ids: Vec<&str> = picker
        .visible_session_iter()
        .map(|session| session.id.as_str())
        .collect();
    assert_eq!(visible_ids, vec!["saved_here", "saved_elsewhere"]);
    let headers: Vec<bool> = picker
        .items
        .iter()
        .map(|item| {
            matches!(item, PickerItem::CurrentDirHeader { .. })
                || matches!(item, PickerItem::SavedHeader { .. })
        })
        .collect();
    // header, pinned row, saved header, saved row
    assert_eq!(
        headers,
        vec![true, false, true, false],
        "expected the pinned-group and saved headers, in that order"
    );
    assert!(matches!(
        picker.items[0],
        PickerItem::CurrentDirHeader {
            session_count: 1,
            ..
        }
    ));
    assert!(matches!(
        picker.items[2],
        PickerItem::SavedHeader { session_count: 1 }
    ));
}

#[test]
fn test_current_dir_group_follows_search_and_skips_grouped_views() {
    let mut here_a = make_session("here_a", "here-a", false, SessionStatus::Closed);
    here_a.working_dir = Some("/home/jeremy/project".to_string());
    let mut here_b = make_session("here_b", "here-b", false, SessionStatus::Closed);
    here_b.working_dir = Some("/home/jeremy/project".to_string());

    let mut picker = SessionPicker::new(vec![here_a.clone(), here_b.clone()]);
    picker.set_current_dir(Some("/home/jeremy/project".to_string()));

    // Searching narrows the pinned group instead of bypassing it. The search
    // index is built at construction, so search by the session id.
    start_search(&mut picker, "here_a");
    assert_eq!(picker.visible_sessions.len(), 1);
    assert_eq!(
        picker.visible_session_iter().next().map(|s| s.id.as_str()),
        Some("here_a")
    );
    assert!(matches!(
        picker.items.first(),
        Some(PickerItem::CurrentDirHeader { .. })
    ));

    // Filter modes other than All are flat lists: no pinned header there.
    picker.search_active = false;
    picker.search_query.clear();
    picker.cycle_filter_mode();
    assert_eq!(picker.filter_mode, SessionFilterMode::Latest);
    picker.cycle_filter_mode();
    assert_eq!(picker.filter_mode, SessionFilterMode::CurrentDir);
    assert!(
        picker
            .items
            .iter()
            .all(|item| matches!(item, PickerItem::Session))
    );
}

#[test]
fn test_current_dir_group_pinned_across_server_groups_and_orphans() {
    let mut grouped_here =
        make_session("grouped_here", "grouped-here", false, SessionStatus::Closed);
    grouped_here.working_dir = Some("/home/jeremy/project".to_string());
    grouped_here.last_message_time = Utc::now() - ChronoDuration::minutes(1);
    let mut orphan_here = make_session("orphan_here", "orphan-here", false, SessionStatus::Closed);
    orphan_here.working_dir = Some("/home/jeremy/project/".to_string());
    orphan_here.last_message_time = Utc::now() - ChronoDuration::minutes(5);
    let mut orphan_elsewhere = make_session(
        "orphan_elsewhere",
        "orphan-elsewhere",
        false,
        SessionStatus::Closed,
    );
    orphan_elsewhere.working_dir = Some("/home/jeremy/elsewhere".to_string());
    orphan_elsewhere.last_message_time = Utc::now() - ChronoDuration::minutes(10);

    let groups = vec![ServerGroup {
        name: "main".to_string(),
        icon: "🛰".to_string(),
        version: "v0.1.0".to_string(),
        git_hash: "abc1234".to_string(),
        is_running: true,
        sessions: vec![grouped_here.clone()],
    }];

    let mut picker =
        SessionPicker::new_grouped(groups, vec![orphan_here.clone(), orphan_elsewhere.clone()]);
    picker.set_current_dir(Some("/home/jeremy/project".to_string()));
    picker.rebuild_items();

    // Pinned group first; the current-dir rows are pulled out of their
    // original sections (the server group becomes empty and is skipped), then
    // the remaining orphans under their header.
    let visible_ids: Vec<&str> = picker
        .visible_session_iter()
        .map(|session| session.id.as_str())
        .collect();
    assert_eq!(
        visible_ids,
        vec!["grouped_here", "orphan_here", "orphan_elsewhere"]
    );
    assert!(matches!(
        picker.items[0],
        PickerItem::CurrentDirHeader {
            session_count: 2,
            ..
        }
    ));
    assert!(matches!(
        picker.items[3],
        PickerItem::OrphanHeader { session_count: 1 }
    ));
}

#[test]
fn test_short_dir_label_collapses_long_paths_to_basename() {
    // Short paths render in full.
    assert_eq!(
        short_dir_label("/home/jeremy/project"),
        "/home/jeremy/project"
    );
    // Long home-style paths collapse to ~/basename.
    let long_home = "/Users/jeremy/Documents/deeply/nested/projects/jcode";
    assert_eq!(short_dir_label(long_home), "~/jcode");
    let long_home2 = "/home/jeremy/Documents/deeply/nested/projects/jcode";
    assert_eq!(short_dir_label(long_home2), "~/jcode");
    // Long non-home paths collapse to /…/basename.
    let long_other = "/srv/workspace/very/deeply/nested/project";
    assert_eq!(short_dir_label(long_other), "/…/project");
}

#[test]
fn test_reseed_grouped_keeps_pinned_group_and_selection() {
    // Simulate the async `/resume` refresh: the cached list renders first,
    // then the freshly loaded session data lands via `reseed_grouped`. The
    // pinned group must survive the swap and the user's selection with it.
    let mut here = make_session("here", "here", false, SessionStatus::Closed);
    here.working_dir = Some("/home/jeremy/project".to_string());
    let mut elsewhere = make_session("elsewhere", "elsewhere", false, SessionStatus::Closed);
    elsewhere.working_dir = Some("/home/jeremy/elsewhere".to_string());

    let mut picker = SessionPicker::new_grouped(Vec::new(), vec![here, elsewhere]);
    picker.set_current_dir(Some("/home/jeremy/project".to_string()));
    picker.rebuild_items();
    assert!(matches!(
        picker.items.first(),
        Some(PickerItem::CurrentDirHeader { .. })
    ));

    // Select the pinned session, then reseed with an extended list (the
    // refresh adds a second current-dir session and a new orphan).
    let pinned_idx = picker
        .item_to_session
        .iter()
        .position(|slot| slot.is_some())
        .expect("selectable item");
    picker.list_state.select(Some(pinned_idx));
    assert_eq!(
        picker.selected_session().map(|s| s.id.as_str()),
        Some("here")
    );

    let mut here2 = make_session("here2", "here2", false, SessionStatus::Closed);
    here2.working_dir = Some("/home/jeremy/project".to_string());
    let mut other = make_session("other", "other", false, SessionStatus::Closed);
    other.working_dir = Some("/home/jeremy/other".to_string());
    picker.reseed_grouped(Vec::new(), vec![here2, other]);

    // Pinned group still on top; the reseeded list has only one
    // current-dir session (here2), so the header count reflects that.
    assert!(matches!(
        picker.items.first(),
        Some(PickerItem::CurrentDirHeader {
            session_count: 1,
            ..
        })
    ));
    // The selected session survived by id ("here" is no longer in the list,
    // so selection falls back to the first selectable item).
    let selected_id = picker
        .selected_session()
        .map(|s| s.id.as_str())
        .expect("a selection exists after reseed");
    assert!(
        ["here", "here2"].contains(&selected_id),
        "selection should resolve to a current-dir session, got {selected_id}"
    );
}

#[test]
fn test_current_dir_group_renders_on_first_paint_without_extra_rebuild() {
    // The pick_session / open_session_picker order: construct, then set the
    // current dir. The pinned group must be in the items immediately, with no
    // extra rebuild call, so the first frame shows it (regression: the group
    // used to appear only after some unrelated event rebuilt the list).
    let mut here = make_session("here", "here", false, SessionStatus::Closed);
    here.working_dir = Some("/home/jeremy/project".to_string());
    let mut elsewhere = make_session("elsewhere", "elsewhere", false, SessionStatus::Closed);
    elsewhere.working_dir = Some("/home/jeremy/elsewhere".to_string());

    let mut picker = SessionPicker::new_grouped(Vec::new(), vec![here.clone(), elsewhere.clone()]);
    picker.set_current_dir(Some("/home/jeremy/project".to_string()));

    assert!(matches!(
        picker.items.first(),
        Some(PickerItem::CurrentDirHeader {
            session_count: 1,
            ..
        })
    ));
    let visible_ids: Vec<&str> = picker
        .visible_session_iter()
        .map(|session| session.id.as_str())
        .collect();
    assert_eq!(visible_ids, vec!["here", "elsewhere"]);

    // Setting the same dir again (after normalization) is a no-op: no
    // rebuild churn, the pinned group stays as-is.
    picker.set_current_dir(Some("/home/jeremy/project/".to_string()));
    assert_eq!(picker.items.len(), 3);
    picker.set_current_dir(None);
    assert!(
        picker
            .items
            .iter()
            .all(|item| matches!(item, PickerItem::Session))
    );
    assert_eq!(picker.items.len(), 2);
}

#[test]
fn test_current_dir_group_sorts_running_sessions_first_within_header() {
    // The pinned group keeps the running-state triage: a current-dir session
    // with a live process sits above current-dir ones that are not running,
    // even when it is older, and the sections below the header still show the
    // stopped remainder in recency order.
    let mut stopped_here_new = make_session(
        "stopped_here_new",
        "stopped-here-new",
        false,
        SessionStatus::Closed,
    );
    stopped_here_new.working_dir = Some("/home/jeremy/project".to_string());
    stopped_here_new.last_message_time = Utc::now() - ChronoDuration::minutes(1);
    let mut ready_here_old = make_session(
        "ready_here_old",
        "ready-here-old",
        false,
        SessionStatus::Active,
    );
    ready_here_old.working_dir = Some("/home/jeremy/project/".to_string());
    ready_here_old.last_message_time = Utc::now() - ChronoDuration::hours(3);
    let mut stopped_elsewhere = make_session(
        "stopped_elsewhere",
        "stopped-elsewhere",
        false,
        SessionStatus::Closed,
    );
    stopped_elsewhere.working_dir = Some("/home/jeremy/elsewhere".to_string());
    stopped_elsewhere.last_message_time = Utc::now() - ChronoDuration::hours(2);

    let mut picker = SessionPicker::new(vec![
        stopped_here_new.clone(),
        ready_here_old.clone(),
        stopped_elsewhere.clone(),
    ]);
    picker.set_current_dir(Some("/home/jeremy/project".to_string()));
    picker.set_live_presence_for_test(vec![live_presence("ready_here_old", false)]);

    let visible_ids: Vec<&str> = picker
        .visible_session_iter()
        .map(|session| session.id.as_str())
        .collect();
    // Ready (live, done streaming) pins first inside the header, then the
    // newer stopped current-dir session, then the outside session.
    assert_eq!(
        visible_ids,
        vec!["ready_here_old", "stopped_here_new", "stopped_elsewhere"]
    );
    assert!(matches!(
        picker.items.first(),
        Some(PickerItem::CurrentDirHeader {
            session_count: 2,
            ..
        })
    ));
}

#[test]
fn test_saved_section_sorts_running_sessions_first() {
    // Saved sessions keep their own section (nothing pins to the current dir
    // here), and within it a running one floats above a newer stopped one.
    let mut saved_stopped_new = make_session(
        "saved_stopped_new",
        "saved-stopped-new",
        false,
        SessionStatus::Closed,
    );
    saved_stopped_new.saved = true;
    saved_stopped_new.last_message_time = Utc::now() - ChronoDuration::minutes(1);
    let mut saved_working_old = make_session(
        "saved_working_old",
        "saved-working-old",
        false,
        SessionStatus::Active,
    );
    saved_working_old.saved = true;
    saved_working_old.last_message_time = Utc::now() - ChronoDuration::hours(2);

    let mut picker = SessionPicker::new(vec![saved_stopped_new, saved_working_old]);
    picker.set_live_presence_for_test(vec![live_presence("saved_working_old", true)]);

    let visible_ids: Vec<&str> = picker
        .visible_session_iter()
        .map(|session| session.id.as_str())
        .collect();
    assert_eq!(visible_ids, vec!["saved_working_old", "saved_stopped_new"]);
    assert!(matches!(
        picker.items.first(),
        Some(PickerItem::SavedHeader { session_count: 2 })
    ));
}

#[test]
fn test_all_view_sorts_running_sessions_above_stopped() {
    let stopped_new = make_session("stopped_new", "stopped-new", false, SessionStatus::Closed);
    let mut working_old = make_session("working_old", "working-old", false, SessionStatus::Active);
    working_old.last_message_time = Utc::now() - ChronoDuration::hours(2);
    let mut ready_old = make_session("ready_old", "ready-old", false, SessionStatus::Active);
    ready_old.last_message_time = Utc::now() - ChronoDuration::hours(3);
    let mut stopped_old = make_session("stopped_old", "stopped-old", false, SessionStatus::Closed);
    stopped_old.last_message_time = Utc::now() - ChronoDuration::hours(4);

    // Before presence lands the list is newest-first:
    // stopped_new, working_old, ready_old, stopped_old.
    let mut picker = SessionPicker::new(vec![
        stopped_new.clone(),
        working_old.clone(),
        ready_old.clone(),
        stopped_old.clone(),
    ]);
    picker.set_live_presence_for_test(vec![
        live_presence("working_old", true),
        live_presence("ready_old", false),
    ]);

    let visible: Vec<&str> = picker
        .visible_session_iter()
        .map(|session| session.id.as_str())
        .collect();
    // Running sessions float above stopped ones in the All view too, ready
    // above working, each triage group still newest-first.
    assert_eq!(
        visible,
        vec!["ready_old", "working_old", "stopped_new", "stopped_old"]
    );
}

#[test]
fn test_running_saved_current_dir_session_pins_once_and_sorts_in_group() {
    // Cross-feature edge: a session that is saved, in the current dir, AND
    // running renders exactly once (pinned group), the running triage applies
    // inside the pinned group, and the saved section never picks it up.
    let mut working_here =
        make_session("working_here", "working-here", false, SessionStatus::Active);
    working_here.working_dir = Some("/home/jeremy/project".to_string());
    working_here.saved = true;
    let mut stopped_here_new = make_session(
        "stopped_here_new",
        "stopped-here-new",
        false,
        SessionStatus::Closed,
    );
    stopped_here_new.working_dir = Some("/home/jeremy/project".to_string());
    stopped_here_new.last_message_time = Utc::now();
    let mut stopped_here_old = make_session(
        "stopped_here_old",
        "stopped-here-old",
        false,
        SessionStatus::Closed,
    );
    stopped_here_old.working_dir = Some("/home/jeremy/project".to_string());
    stopped_here_old.last_message_time = Utc::now() - ChronoDuration::hours(1);
    let mut saved_elsewhere = make_session(
        "saved_elsewhere",
        "saved-elsewhere",
        false,
        SessionStatus::Closed,
    );
    saved_elsewhere.working_dir = Some("/home/jeremy/elsewhere".to_string());
    saved_elsewhere.saved = true;

    let mut picker = SessionPicker::new(vec![
        working_here,
        stopped_here_new,
        stopped_here_old,
        saved_elsewhere,
    ]);
    picker.set_current_dir(Some("/home/jeremy/project".to_string()));
    picker.set_live_presence_for_test(vec![live_presence("working_here", true)]);

    let visible: Vec<&str> = picker
        .visible_session_iter()
        .map(|session| session.id.as_str())
        .collect();
    // Pinned group first with the running session on top of it (despite being
    // the oldest), then the saved section without working_here.
    assert_eq!(
        visible,
        vec![
            "working_here",
            "stopped_here_new",
            "stopped_here_old",
            "saved_elsewhere"
        ]
    );
    // The saved header still counts only its own row.
    let saved_header = picker
        .items
        .iter()
        .find_map(|item| match item {
            PickerItem::SavedHeader { session_count } => Some(*session_count),
            _ => None,
        })
        .expect("saved section present");
    assert_eq!(saved_header, 1);
    assert_eq!(
        picker
            .visible_session_iter()
            .filter(|s| s.id == "working_here")
            .count(),
        1
    );
}

#[test]
fn test_working_to_ready_transition_swaps_order_in_place() {
    // The most common real transition: a session finishes streaming but its
    // process stays alive (ready for input). It must move ABOVE still-working
    // sessions without leaving the live triage tier, and the poll rebuild
    // alone drives the swap. The promoted session is the OLDER one so the
    // assertion catches an actual row swap, not a no-op reorder of an
    // already-first row.
    let stopped_new = make_session("stopped_new", "stopped-new", false, SessionStatus::Closed);
    let mut working_old = make_session("working_old", "working-old", false, SessionStatus::Active);
    working_old.last_message_time = Utc::now() - ChronoDuration::hours(1);
    let mut working_new = make_session("working_new", "working-new", false, SessionStatus::Active);
    working_new.last_message_time = Utc::now() - ChronoDuration::minutes(5);

    let mut picker = SessionPicker::new(vec![stopped_new, working_old, working_new]);
    // Both live and streaming: recency order inside the working tier.
    picker.set_live_presence_for_test(vec![
        live_presence("working_old", true),
        live_presence("working_new", true),
    ]);
    let visible: Vec<&str> = picker
        .visible_session_iter()
        .map(|session| session.id.as_str())
        .collect();
    assert_eq!(visible, vec!["working_new", "working_old", "stopped_new"]);

    // The older session finishes its turn first: ready (rank 0) swaps ABOVE
    // the still-working newer session (rank 1). Rows actually move.
    picker.set_live_presence_for_test(vec![
        live_presence("working_old", false),
        live_presence("working_new", true),
    ]);
    let visible: Vec<&str> = picker
        .visible_session_iter()
        .map(|session| session.id.as_str())
        .collect();
    assert_eq!(visible, vec!["working_old", "working_new", "stopped_new"]);

    // The newer one finishes too: both ready, recency order restored between
    // them, the stopped session stays below both live ones.
    picker.set_live_presence_for_test(vec![
        live_presence("working_old", false),
        live_presence("working_new", false),
    ]);
    let visible: Vec<&str> = picker
        .visible_session_iter()
        .map(|session| session.id.as_str())
        .collect();
    assert_eq!(visible, vec!["working_new", "working_old", "stopped_new"]);
}

#[test]
fn test_search_mode_keeps_running_sessions_above_stopped() {
    // Search results run through filtered_session_refs, so the triage applies
    // while searching too: a stopped session matching the query still renders
    // below a running one, with the current-dir group kept.
    let mut working_old = make_session("working_old", "working-old", false, SessionStatus::Active);
    working_old.last_message_time = Utc::now() - ChronoDuration::hours(2);
    let mut stopped_new = make_session("stopped_new", "stopped-new", false, SessionStatus::Closed);
    stopped_new.last_message_time = Utc::now();

    let mut picker = SessionPicker::new(vec![working_old, stopped_new]);
    picker.set_live_presence_for_test(vec![live_presence("working_old", true)]);
    start_search(&mut picker, "session");

    let visible: Vec<&str> = picker
        .visible_session_iter()
        .map(|session| session.id.as_str())
        .collect();
    assert_eq!(visible, vec!["working_old", "stopped_new"]);
}

#[test]
fn test_working_view_keeps_recency_order_within_streaming_sessions() {
    // The Working view only shows streaming sessions; the triage collapses
    // to a no-op there (every row rank 1), so it must not scramble the
    // recency order with an unstable comparator behind a sort_by_key that
    // allocates fresh rank values per comparison pair.
    let mut streaming_old = make_session(
        "streaming_old",
        "streaming-old",
        false,
        SessionStatus::Active,
    );
    streaming_old.last_message_time = Utc::now() - ChronoDuration::hours(2);
    let mut streaming_new = make_session(
        "streaming_new",
        "streaming-new",
        false,
        SessionStatus::Active,
    );
    streaming_new.last_message_time = Utc::now();

    let mut picker = SessionPicker::new(vec![streaming_old, streaming_new]);
    picker.set_live_presence_for_test(vec![
        live_presence("streaming_old", true),
        live_presence("streaming_new", true),
    ]);
    picker.filter_mode = SessionFilterMode::Working;
    picker.rebuild_items();

    let visible: Vec<&str> = picker
        .visible_session_iter()
        .map(|session| session.id.as_str())
        .collect();
    assert_eq!(visible, vec!["streaming_new", "streaming_old"]);
}

#[test]
fn test_active_view_tries_ready_above_working_matches_legacy_triage() {
    // The old Active view sorted ready sessions above working ones inside the
    // live set. The generalized rank keeps that exact semantic: rank 0
    // (ready) above rank 1 (working), stopped sessions invisible in this
    // view regardless of recency.
    let mut working_new = make_session("working_new", "working-new", false, SessionStatus::Active);
    working_new.last_message_time = Utc::now();
    let mut ready_old = make_session("ready_old", "ready-old", false, SessionStatus::Active);
    ready_old.last_message_time = Utc::now() - ChronoDuration::hours(1);
    let mut stopped_newest = make_session(
        "stopped_newest",
        "stopped-newest",
        false,
        SessionStatus::Closed,
    );
    stopped_newest.last_message_time = Utc::now() + ChronoDuration::minutes(5);

    let mut picker = SessionPicker::new(vec![working_new, ready_old, stopped_newest]);
    picker.set_live_presence_for_test(vec![
        live_presence("working_new", true),
        live_presence("ready_old", false),
    ]);
    picker.filter_mode = SessionFilterMode::Active;
    picker.rebuild_items();

    let visible: Vec<&str> = picker
        .visible_session_iter()
        .map(|session| session.id.as_str())
        .collect();
    assert_eq!(visible, vec!["ready_old", "working_new"]);
}

#[test]
fn test_grouped_refs_rank_resolution_matches_flat() {
    // The triage resolves ranks through session_by_ref, which must dereference
    // SessionRef::Group rows (server sessions) the same as flat ones: a stopped
    // session in a server group stays below a running one, inside the same
    // group.
    let mut working_old = make_session("working_old", "working-old", false, SessionStatus::Active);
    working_old.last_message_time = Utc::now() - ChronoDuration::hours(2);
    let mut stopped_new = make_session("stopped_new", "stopped-new", false, SessionStatus::Closed);
    stopped_new.last_message_time = Utc::now();

    let groups = vec![ServerGroup {
        name: "main".to_string(),
        icon: "🛰".to_string(),
        version: "v0.1.0".to_string(),
        git_hash: "abc1234".to_string(),
        is_running: true,
        sessions: vec![working_old, stopped_new],
    }];

    let mut picker = SessionPicker::new_grouped(groups, Vec::new());
    picker.set_live_presence_for_test(vec![live_presence("working_old", true)]);

    let visible: Vec<&str> = picker
        .visible_session_iter()
        .map(|session| session.id.as_str())
        .collect();
    assert_eq!(visible, vec!["working_old", "stopped_new"]);
}

#[test]
fn test_saved_filter_view_sorts_running_above_stopped() {
    // The Saved filter mode early-returns in rebuild_items (flat list, no
    // sections), but it still flows through filtered_session_refs, so the
    // triage must hold there too.
    let mut working_old = make_session("working_old", "working-old", false, SessionStatus::Active);
    working_old.last_message_time = Utc::now() - ChronoDuration::hours(2);
    let mut saved_stopped_new = make_session(
        "saved_stopped_new",
        "saved-stopped-new",
        false,
        SessionStatus::Closed,
    );
    saved_stopped_new.last_message_time = Utc::now();
    saved_stopped_new.saved = true;
    working_old.saved = true;

    let mut picker = SessionPicker::new(vec![working_old, saved_stopped_new]);
    picker.set_live_presence_for_test(vec![live_presence("working_old", true)]);
    picker.filter_mode = SessionFilterMode::Saved;
    picker.rebuild_items();

    let visible: Vec<&str> = picker
        .visible_session_iter()
        .map(|session| session.id.as_str())
        .collect();
    assert_eq!(visible, vec!["working_old", "saved_stopped_new"]);
}

#[test]
fn test_multi_select_survives_presence_re_sort() {
    // The user multi-selects sessions, then the 2s presence poll re-sorts the
    // list under them. The selection is tracked by id and retained against the
    // visible set, so it must survive the reorder untouched.
    let mut working_old = make_session("working_old", "working-old", false, SessionStatus::Active);
    working_old.last_message_time = Utc::now() - ChronoDuration::hours(2);
    let mut stopped_new = make_session("stopped_new", "stopped-new", false, SessionStatus::Closed);
    stopped_new.last_message_time = Utc::now();

    let mut picker = SessionPicker::new(vec![working_old, stopped_new]);
    picker.set_live_presence_for_test(vec![live_presence("working_old", true)]);

    // Highlight the bottom row (stopped_new, below the running one) and
    // select it, then also select the top row.
    picker.list_state.select(Some(1));
    picker.toggle_selected_session();
    picker.list_state.select(Some(0));
    picker.toggle_selected_session();
    assert_eq!(picker.selection_count(), 2);

    // Poll: the stopped session starts working (a live process picks it up).
    // It is newer than the other live one, so both now sit in the working
    // tier and recency puts it first: the rows ACTUALLY swap under the
    // selection while the rebuild runs.
    picker.set_live_presence_for_test(vec![
        live_presence("working_old", true),
        live_presence("stopped_new", true),
    ]);
    assert_eq!(picker.selection_count(), 2);
    // Exact IDs, not just the count: the selection must survive the reorder
    // as the same two sessions, not two arbitrary visible ones.
    let mut selected_ids: Vec<&str> = picker
        .selected_session_ids
        .iter()
        .map(String::as_str)
        .collect();
    selected_ids.sort_unstable();
    assert_eq!(selected_ids, vec!["stopped_new", "working_old"]);
    let targets = picker.selection_or_current_targets();
    let ids: Vec<&str> = picker
        .visible_session_iter()
        .map(|s| s.id.as_str())
        .collect();
    assert_eq!(ids, vec!["stopped_new", "working_old"]);
    assert_eq!(targets.len(), 2);
    let mut target_ids: Vec<&str> = targets
        .iter()
        .filter_map(|target| match target {
            ResumeTarget::JcodeSession { session_id } => Some(session_id.as_str()),
            _ => None,
        })
        .collect();
    target_ids.sort_unstable();
    assert_eq!(target_ids, vec!["stopped_new", "working_old"]);
}

#[test]
fn test_all_view_re_sorts_when_presence_changes() {
    let stopped_new = make_session("stopped_new", "stopped-new", false, SessionStatus::Closed);
    let mut working_old = make_session("working_old", "working-old", false, SessionStatus::Active);
    working_old.last_message_time = Utc::now() - ChronoDuration::hours(2);

    let mut picker = SessionPicker::new(vec![stopped_new, working_old]);
    picker.set_live_presence_for_test(vec![live_presence("working_old", true)]);
    let visible: Vec<&str> = picker
        .visible_session_iter()
        .map(|session| session.id.as_str())
        .collect();
    assert_eq!(visible, vec!["working_old", "stopped_new"]);

    // Simulate the stream ending on the 2s presence poll: the fresh snapshot
    // (no live owner for these test ids) re-sorts the All view back to plain
    // recency, so the running-state order tracks reality while the picker
    // idles.
    picker.live_presence_refreshed_at = None;
    assert!(picker.maybe_refresh_live_presence());
    let visible: Vec<&str> = picker
        .visible_session_iter()
        .map(|session| session.id.as_str())
        .collect();
    assert_eq!(visible, vec!["stopped_new", "working_old"]);
}

#[test]
fn test_rendered_all_view_paints_running_rows_above_stopped() {
    // End-to-end through the real draw path: not just the backing order of
    // `visible_sessions`, but what the user actually sees on screen. The
    // streaming row (and the pinned current-dir header above it) must paint
    // before the stopped rows, and the working badge must actually render.
    let mut stopped_here =
        make_session("stopped_here", "stopped-here", false, SessionStatus::Closed);
    stopped_here.working_dir = Some("/home/jeremy/project".to_string());
    stopped_here.last_message_time = Utc::now() - ChronoDuration::minutes(1);
    let mut working_here =
        make_session("working_here", "working-here", false, SessionStatus::Active);
    working_here.working_dir = Some("/home/jeremy/project/".to_string());
    working_here.last_message_time = Utc::now() - ChronoDuration::hours(3);
    let mut stopped_elsewhere = make_session(
        "stopped_elsewhere",
        "stopped-elsewhere",
        false,
        SessionStatus::Closed,
    );
    stopped_elsewhere.working_dir = Some("/home/jeremy/elsewhere".to_string());
    stopped_elsewhere.last_message_time = Utc::now() - ChronoDuration::hours(2);
    // Unique row titles so the painted screen can be searched per session.
    stopped_here.title = "Stopped here row".to_string();
    working_here.title = "Working here row".to_string();
    stopped_elsewhere.title = "Elsewhere row".to_string();

    let mut picker = SessionPicker::new(vec![
        stopped_here.clone(),
        working_here.clone(),
        stopped_elsewhere.clone(),
    ]);
    picker.set_current_dir(Some("/home/jeremy/project".to_string()));
    picker.set_live_presence_for_test(vec![live_presence("working_here", true)]);

    let text = buffer_text(&mut picker, 120, 40);
    let working_pos = text.find("Working here row").expect("working row rendered");
    let stopped_here_pos = text
        .find("Stopped here row")
        .expect("stopped-here row rendered");
    let elsewhere_pos = text.find("Elsewhere row").expect("elsewhere row rendered");
    // The pinned header paints above the rows in it, the streaming session
    // paints first inside the header, and the stopped ones follow. The live
    // badge ("working <duration>") proves the row carries its running state,
    // not just a position.
    assert!(
        working_pos < stopped_here_pos,
        "working session should render above the stopped current-dir one"
    );
    assert!(
        stopped_here_pos < elsewhere_pos,
        "current-dir rows should render above the outside one"
    );
    assert!(
        text.contains("working 1m"),
        "expected the working badge with duration on screen"
    );
}

#[test]
fn test_presence_rebuild_keeps_preview_scroll_position() {
    // A background session starting or finishing triggers a presence-driven
    // rebuild. The highlighted session survives it by id, so the preview
    // scroll position must survive too: without the save/restore in
    // `maybe_refresh_live_presence`, `rebuild_items` resets
    // `scroll_offset`/`auto_scroll_preview` and the next draw yanks the user
    // back to the bottom mid-read.
    let reading = make_session("reading", "reading", false, SessionStatus::Active);
    let mut stopped_old = make_session("stopped_old", "stopped-old", false, SessionStatus::Closed);
    stopped_old.last_message_time = Utc::now() - ChronoDuration::hours(2);
    let mut working_old = make_session("working_old", "working-old", false, SessionStatus::Active);
    working_old.last_message_time = Utc::now() - ChronoDuration::hours(3);

    let mut picker = SessionPicker::new(vec![stopped_old, working_old, reading]);
    // Reading a specific row: the highlight is on `reading` and the preview
    // is scrolled somewhere that is NOT the auto-scroll default.
    picker.set_live_presence_for_test(vec![live_presence("reading", true)]);
    let reading_idx = picker
        .find_item_index_for_session_id("reading")
        .expect("reading row present");
    picker.list_state.select(Some(reading_idx));
    picker.scroll_offset = 7;
    picker.auto_scroll_preview = false;
    *picker.list_state.offset_mut() = 1;

    // Another session goes live and starts streaming: the poll snapshot
    // changes, the rebuild re-sorts, but `reading` stays selected, so the
    // user's scroll position must be preserved.
    picker.live_presence_refreshed_at = None;
    assert!(picker.maybe_refresh_live_presence());
    assert_eq!(
        picker.selected_session().map(|s| s.id.as_str()),
        Some("reading")
    );
    assert_eq!(picker.scroll_offset, 7);
    assert!(!picker.auto_scroll_preview);
    assert_eq!(picker.list_state.offset(), 1);

    // When the selection does NOT survive (the session disappears from the
    // list), the rebuild's defaults apply again.
    picker.scroll_offset = 5;
    picker.auto_scroll_preview = false;
    let mut gone = picker;
    gone.all_sessions.retain(|session| session.id != "reading");
    gone.cached_search_query.clear();
    gone.live_presence_refreshed_at = None;
    gone.rebuild_items();
    assert!(
        gone.selected_session()
            .map(|s| s.id != "reading")
            .unwrap_or(true)
    );
    assert_eq!(gone.scroll_offset, 0);
    assert!(gone.auto_scroll_preview);
}
