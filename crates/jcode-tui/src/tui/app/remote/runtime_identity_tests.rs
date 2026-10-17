use super::{
    parse_release_semver, server_release_is_older_than_client,
    should_defer_history_for_runtime_identity_with_allow,
};

#[test]
fn runtime_identity_gate_defers_stale_server_history_by_default() {
    assert!(should_defer_history_for_runtime_identity_with_allow(
        Some(true),
        false,
        false
    ));
    assert!(!should_defer_history_for_runtime_identity_with_allow(
        Some(false),
        false,
        false
    ));
    assert!(!should_defer_history_for_runtime_identity_with_allow(
        None, false, false
    ));
}

#[test]
fn runtime_identity_gate_allows_explicit_mismatch_escape_hatch() {
    assert!(!should_defer_history_for_runtime_identity_with_allow(
        Some(true),
        false,
        true
    ));
    assert!(!should_defer_history_for_runtime_identity_with_allow(
        None, true, true
    ));
}

#[test]
fn client_detected_older_server_always_defers() {
    // Ancient server (server_has_update: None) that the client independently
    // measured as older -> defer. This is the issue #295 macOS case where a
    // pre-self-heal daemon can never set server_has_update itself.
    assert!(should_defer_history_for_runtime_identity_with_allow(
        None, true, false
    ));
    // A server that self-reports "no newer binary" (Some(false)) but that the
    // client can PROVE is an older release -> still defer. The daemon's
    // self-report is locally correct (its own shared-server channel points at
    // its old build) but globally wrong; the newer client is authoritative.
    // This is the "current client, stale server" report: trusting Some(false)
    // here is exactly what left the server stuck on the old version forever.
    assert!(should_defer_history_for_runtime_identity_with_allow(
        Some(false),
        true,
        false
    ));
    // Same-release/newer server (client could not prove it is older) that
    // self-reports "no newer binary" -> trust it, do not force a reload loop.
    assert!(!should_defer_history_for_runtime_identity_with_allow(
        Some(false),
        false,
        false
    ));
}

#[test]
fn parse_release_semver_refuses_unorderable_dev_builds() {
    assert_eq!(parse_release_semver("v0.17.0 (d741696f)"), Some((0, 17, 0)));
    assert_eq!(parse_release_semver("0.14.2"), Some((0, 14, 2)));
    // Dev/dirty builds share a base semver and must not be ordered.
    assert_eq!(parse_release_semver("v0.18.4-dev (102e9750, dirty)"), None);
    assert_eq!(parse_release_semver("v0.14.2-dev (38452185, dirty)"), None);
    assert_eq!(parse_release_semver("unknown"), None);
}

#[test]
fn server_release_older_than_client_is_selfdev_safe() {
    // Clean release older than clean client -> stale.
    assert!(server_release_is_older_than_client(
        Some("v0.14.2 (38452185)"),
        "v0.17.0 (d741696f)"
    ));
    // Equal or newer -> not stale.
    assert!(!server_release_is_older_than_client(
        Some("v0.17.0"),
        "v0.17.0"
    ));
    assert!(!server_release_is_older_than_client(
        Some("v0.18.0"),
        "v0.17.0"
    ));
    // Either side dev/dirty/unparseable -> never claim staleness (protects
    // self-dev and branched daemons from a forced downgrade).
    assert!(!server_release_is_older_than_client(
        Some("v0.14.2-dev (abc, dirty)"),
        "v0.17.0"
    ));
    assert!(!server_release_is_older_than_client(
        Some("v0.14.2"),
        "v0.17.0-dev (abc, dirty)"
    ));
    assert!(!server_release_is_older_than_client(None, "v0.17.0"));
}
