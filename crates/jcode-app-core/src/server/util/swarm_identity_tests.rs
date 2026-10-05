use super::default_swarm_id_for_session;

#[test]
fn independent_root_sessions_have_distinct_swarm_ids() {
    assert_eq!(
        default_swarm_id_for_session("session-one").as_deref(),
        Some("session:session-one")
    );
    assert_eq!(
        default_swarm_id_for_session("session-two").as_deref(),
        Some("session:session-two")
    );
    assert_ne!(
        default_swarm_id_for_session("session-one"),
        default_swarm_id_for_session("session-two")
    );
}

#[test]
fn empty_session_cannot_own_a_swarm() {
    assert_eq!(default_swarm_id_for_session("  "), None);
}
