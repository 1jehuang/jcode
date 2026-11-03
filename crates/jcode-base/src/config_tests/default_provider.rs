//! Synchronization of externally edited provider preferences and account pools.

use super::*;

#[test]
fn external_default_edit_syncs_pool_without_reordering_on_unrelated_reload() {
    let _guard = crate::storage::lock_test_env();
    let prev_home = std::env::var_os("JCODE_HOME");
    let dir = tempfile::TempDir::new().unwrap();
    crate::env::set_var("JCODE_HOME", dir.path());
    Config::invalidate_cache();
    let path = Config::path().unwrap();
    std::fs::write(&path, "[provider]\ndefault_provider = 'openai-oauth'\n").unwrap();
    assert_eq!(
        crate::config::config().provider.default_provider.as_deref(),
        Some("openai-oauth")
    );
    let mut pool = crate::auth::account_pool::AccountPool::default();
    pool.members = vec!["openai:work".into(), "claude:personal".into()];
    pool.save().unwrap();

    // A raw file edit, not the preference setter, must synchronize startup.
    std::fs::write(
        &path,
        "[provider]\ndefault_provider = 'claude'\ndefault_model = 'claude-opus-5-5'\n",
    )
    .unwrap();
    Config::invalidate_cache();
    assert_eq!(
        crate::config::config().provider.default_provider.as_deref(),
        Some("claude")
    );
    assert_eq!(
        crate::auth::account_pool::AccountPool::load().members[0],
        "claude:personal"
    );

    pool.save().unwrap();
    std::fs::write(&path, "[provider]\ndefault_provider = 'claude'\ndefault_model = 'claude-opus-5-5'\nanthropic_reasoning_effort = 'low'\n").unwrap();
    Config::invalidate_cache();
    assert_eq!(
        crate::config::config()
            .provider
            .anthropic_reasoning_effort
            .as_deref(),
        Some("low")
    );
    assert_eq!(crate::auth::account_pool::AccountPool::load(), pool);
    restore_env_var("JCODE_HOME", prev_home);
    Config::invalidate_cache();
}
