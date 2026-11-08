use super::reload_marker_policy;
use super::reload_state::ForeignMarkerPolicy;

/// Only the shared socket's server may clear a reload marker owned by
/// another process; a custom-socket server preserves it, because clearing
/// would erase the shared daemon's in-flight `Starting` state (review of
/// #1768). Both the bind-time clear and the socket-ready publish consult
/// this. The publish path was missed on the first attempt and reopened
/// the hole. The policy says nothing about a server's *own* marker, which
/// it must always be free to advance.
#[test]
fn only_shared_socket_server_may_clear_a_foreign_reload_marker() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let prev_runtime = std::env::var_os("JCODE_RUNTIME_DIR");
    let runtime_dir = std::fs::canonicalize(temp.path()).expect("canonicalize tempdir");
    crate::env::set_var("JCODE_RUNTIME_DIR", &runtime_dir);

    assert_eq!(
        reload_marker_policy(&runtime_dir.join("jcode.sock")),
        ForeignMarkerPolicy::MayClear
    );
    assert_eq!(
        reload_marker_policy(&runtime_dir.join("run-isolated.sock")),
        ForeignMarkerPolicy::Preserve
    );

    if let Some(prev_runtime) = prev_runtime {
        crate::env::set_var("JCODE_RUNTIME_DIR", prev_runtime);
    } else {
        crate::env::remove_var("JCODE_RUNTIME_DIR");
    }
}
