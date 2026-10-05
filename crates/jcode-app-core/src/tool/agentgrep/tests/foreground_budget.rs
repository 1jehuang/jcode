use super::*;
#[tokio::test]
async fn foreground_budget_returns_fast_search_result_directly() {
    let handle = tokio::spawn(async { Ok(ToolOutput::new("fast result")) });
    let output = await_or_background_search(
        handle,
        std::time::Duration::from_secs(1),
        "fast search".to_string(),
        "agentgrep-fast-test".to_string(),
    )
    .await
    .expect("fast search should complete in foreground");

    assert_eq!(output.output, "fast result");
    assert!(output.metadata.is_none());
}

#[tokio::test]
async fn foreground_budget_promotes_slow_search_without_cancelling_it() {
    let handle = tokio::spawn(async {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        Ok(ToolOutput::new("eventual search result"))
    });
    let output = await_or_background_search(
        handle,
        std::time::Duration::from_millis(1),
        "slow search".to_string(),
        "agentgrep-slow-test".to_string(),
    )
    .await
    .expect("slow search should be promoted");
    let metadata = output.metadata.expect("expected background metadata");

    assert_eq!(metadata["background"], true);
    assert_eq!(metadata["timeout_promoted"], true);
    assert_eq!(metadata["foreground_timeout_ms"], 1);
    assert!(output.output.contains("continuing in background"));

    // The adopted handle must remain alive after the foreground call returns.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let output_path = metadata["output_file"].as_str().expect("output path");
    let saved = tokio::fs::read_to_string(output_path)
        .await
        .expect("background manager should persist the eventual result");
    assert!(saved.contains("eventual search result"));
}
