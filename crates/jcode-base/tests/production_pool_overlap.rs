#![cfg(all(windows, feature = "test-support"))]

#[path = "support/pool_overlap_case.rs"]
mod pool_overlap_case;

#[tokio::test]
#[ignore = "run only under tests/support/windows_mcp/atomic_controller.py"]
async fn overlapping_release_reconnect_preserves_successor_ownership() {
    pool_overlap_case::run(false).await;
}
