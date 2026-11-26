#[test]
fn standard_openrouter_catalog_refresh_skips_without_key() {
    with_clean_provider_test_env(|| {
        let runtime = enter_test_runtime();
        runtime.block_on(async {
            // No OPENROUTER_API_KEY configured: the refresh must not be
            // scheduled regardless of cache state.
            assert!(
                !openrouter::maybe_schedule_standard_openrouter_catalog_refresh(
                    "unit test missing key"
                ),
                "standard OpenRouter refresh must be skipped when no key is configured"
            );
        });
    });
}
