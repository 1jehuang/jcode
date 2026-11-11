#![cfg(all(windows, feature = "test-support"))]

#[path = "support/pool_overlap_case.rs"]
mod pool_overlap_case;

#[tokio::test]
#[ignore = "observational probe; run only under the dedicated atomic fixture controller"]
async fn accounting_and_spawn_samples_do_not_prove_strict_completion() {
    pool_overlap_case::run(true).await;
}
