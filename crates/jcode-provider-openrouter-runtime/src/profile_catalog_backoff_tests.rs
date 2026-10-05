use super::{MODEL_CATALOG_REFRESH_RETRY_SECS, profile_catalog_retry_delay_secs};

#[test]
fn healthy_profile_uses_base_retry_interval() {
    assert_eq!(
        profile_catalog_retry_delay_secs(0),
        MODEL_CATALOG_REFRESH_RETRY_SECS
    );
}

#[test]
fn repeated_failures_back_off_exponentially_and_cap() {
    assert_eq!(
        profile_catalog_retry_delay_secs(1),
        MODEL_CATALOG_REFRESH_RETRY_SECS * 2
    );
    assert_eq!(
        profile_catalog_retry_delay_secs(3),
        MODEL_CATALOG_REFRESH_RETRY_SECS * 8
    );
    // Capped at one hour no matter how many failures accumulate.
    assert_eq!(profile_catalog_retry_delay_secs(20), 60 * 60);
}
