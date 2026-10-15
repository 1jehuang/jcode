use super::*;

#[test]
fn missing_cache_is_an_expected_miss() {
    let temp = tempfile::tempdir().expect("temporary cache directory");

    assert!(read_cached_snapshot(&temp.path().join("missing.json")).is_none());
}

#[test]
fn valid_cache_retains_observation_timestamp() {
    let temp = tempfile::tempdir().expect("temporary cache directory");
    let path = temp.path().join("catalog.json");
    let snapshot = Snapshot {
        version: 1,
        observed_at: 42,
        ..Default::default()
    };
    crate::storage::write_json(&path, &snapshot).expect("write valid catalog");

    assert_eq!(
        read_cached_snapshot(&path)
            .expect("valid cache observation")
            .observed_at,
        42
    );
}

#[test]
fn corrupt_cache_is_a_diagnosed_miss() {
    let temp = tempfile::tempdir().expect("temporary cache directory");
    let path = temp.path().join("catalog.json");
    std::fs::write(&path, b"{invalid json").expect("write corrupt catalog");

    assert!(read_cached_snapshot(&path).is_none());
}

#[test]
fn unreadable_cache_is_a_diagnosed_miss() {
    let temp = tempfile::tempdir().expect("temporary cache directory");

    assert!(read_cached_snapshot(temp.path()).is_none());
}
