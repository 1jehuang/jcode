use super::*;

fn cloud_swan() -> SessionInfo {
    let mut session = make_session("cloud-swan", "swan", false, SessionStatus::Closed);
    session.title = "handoff".into();
    session.working_dir = Some("/home/jeremy/justrust-clean-room".into());
    session.server_icon = Some("☁".into());
    session.server_name = Some("cloud: jcode-cloud".into());
    // Identity must remain searchable even with an incomplete/stale index.
    session.search_index = "handoff".into();
    session.last_message_time -= chrono::Duration::days(3);
    session
}

#[test]
fn resume_identity_search_prioritizes_cloud_swan_over_mentions_before_and_after_worker() {
    let swan = cloud_swan();
    let mut mention = make_session("mention", "tiger", false, SessionStatus::Closed);
    mention.title = "How do I find the swan cloud session?".into();
    mention.first_user_prompt = Some("swan cloud".into());
    mention.search_index = "how do i find the swan cloud session? swan cloud".into();
    let mut local_swan = make_session("local-swan", "swan", false, SessionStatus::Closed);
    local_swan.search_index = "swan cloud mentioned in tool output".into();
    let mut picker = SessionPicker::new(vec![mention, local_swan, swan]);
    for query in [
        "swan cloud",
        "cloud swan",
        "SWAN CLOUD",
        "justrust cloud",
        "handoff",
    ] {
        picker.search_query = query.into();
        picker.rebuild_items();
        assert_eq!(
            picker
                .session_by_ref(picker.visible_sessions[0])
                .unwrap()
                .id,
            "cloud-swan",
            "{query}"
        );
        wait_for_picker_search(&mut picker);
        assert_eq!(
            picker
                .session_by_ref(picker.visible_sessions[0])
                .unwrap()
                .id,
            "cloud-swan",
            "{query}"
        );
    }
    picker.search_query = "swan cloud".into();
    picker.rebuild_items();
    wait_for_picker_search(&mut picker);
    assert_eq!(
        picker.visible_sessions.len(),
        3,
        "keep incidental matches accessible"
    );
    picker.search_query.clear();
    picker.rebuild_items();
    assert_ne!(
        picker
            .session_by_ref(picker.visible_sessions[0])
            .unwrap()
            .id,
        "cloud-swan",
        "unfiltered recency is unchanged"
    );
}

#[test]
fn resume_identity_search_does_not_promote_partial_identity_or_fake_cloud_mentions() {
    let mut local = make_session("local", "swan", false, SessionStatus::Closed);
    local.title = "cloud migration notes".into();
    let cloud = cloud_swan();
    let tokens = loading::search_query_tokens("swan cloud");
    assert!(
        super::super::identity_search::rank(&cloud, &tokens)
            > super::super::identity_search::rank(&local, &tokens)
    );
    let unrelated = loading::search_query_tokens("swan battery");
    assert_eq!(
        super::super::identity_search::rank(&cloud, &unrelated),
        (false, false, false)
    );
}
