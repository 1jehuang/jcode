use super::*;

#[tokio::test(start_paused = true)]
async fn outer_handoff_owner_receives_real_search_results_without_nested_promotion() {
    use std::time::Duration;
    let start = tokio::time::Instant::now();
    let first = tokio::spawn(async {
        tokio::time::sleep(Duration::from_secs(8)).await;
        Ok(ToolOutput::new("first search result"))
    });
    let second = tokio::spawn(async {
        tokio::time::sleep(Duration::from_secs(9)).await;
        Ok(ToolOutput::new("second search result"))
    });
    let (first, second) = crate::tool::handoff::scope(async {
        tokio::join!(
            await_or_background_search(
                first,
                Duration::from_secs(5),
                "first".into(),
                "outer-search-first".into()
            ),
            await_or_background_search(
                second,
                Duration::from_secs(5),
                "second".into(),
                "outer-search-second".into()
            ),
        )
    })
    .await;
    assert_eq!(start.elapsed(), Duration::from_secs(9));
    for (result, expected) in [
        (first, "first search result"),
        (second, "second search result"),
    ] {
        let result = result.unwrap();
        assert_eq!(result.output, expected);
        assert!(
            result.metadata.is_none(),
            "the outer executor owns the only task ID"
        );
    }
}
