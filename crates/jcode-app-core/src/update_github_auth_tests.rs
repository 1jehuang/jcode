use super::*;

#[test]
#[ignore = "live network test"]
fn live_fetch_latest_release_uses_auth() {
    let release = fetch_latest_release_blocking().expect("release fetch should succeed");
    assert!(!release.tag_name.is_empty());
}
