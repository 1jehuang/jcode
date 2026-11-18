use super::remote_auto_server_reload;

#[test]
fn client_reload_policy_suppresses_old_handoff_markers() {
    assert!(!remote_auto_server_reload(true, true, true, true));
    assert!(!remote_auto_server_reload(true, true, true, false));
    assert!(!remote_auto_server_reload(true, true, false, true));
}

#[test]
fn client_reload_policy_preserves_ordinary_startup_and_resume() {
    assert!(remote_auto_server_reload(true, false, false, false));
    assert!(remote_auto_server_reload(true, true, false, false));
    // A leaked marker alone does not identify a hot-exec resume.
    assert!(remote_auto_server_reload(true, false, true, true));
    assert!(remote_auto_server_reload(true, false, true, false));
    assert!(remote_auto_server_reload(true, false, false, true));
}

#[test]
fn client_reload_policy_preserves_disabled_config() {
    for has_resume_target in [false, true] {
        for resuming_handoff in [false, true] {
            for reload_fast_start in [false, true] {
                assert!(!remote_auto_server_reload(
                    false,
                    has_resume_target,
                    resuming_handoff,
                    reload_fast_start,
                ));
            }
        }
    }
}
