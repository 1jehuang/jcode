use super::*;

/// Sets env vars for one test and restores the previous values on drop.
struct EnvGuard(Vec<(String, Option<std::ffi::OsString>)>);

impl EnvGuard {
    fn set(vars: &[(&str, &str)]) -> Self {
        Self(
            vars.iter()
                .map(|(key, value)| {
                    let previous = std::env::var_os(key);
                    jcode_base::env::set_var(key, value);
                    (key.to_string(), previous)
                })
                .collect(),
        )
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, previous) in self.0.drain(..) {
            match previous {
                Some(value) => jcode_base::env::set_var(&key, value),
                None => jcode_base::env::remove_var(&key),
            }
        }
    }
}

fn pool_env(names: &str) -> EnvGuard {
    EnvGuard::set(&[
        ("JCODE_ANTHROPIC_API_BASE", "https://gateway.example/v1"),
        ("JCODE_NAMED_PROVIDER_PROFILE", "gateway"),
        ("JCODE_ANTHROPIC_API_KEY_NAMES", names),
        ("JCODE_ANTHROPIC_API_KEY_NAME", "POOL_KEY_A"),
        ("POOL_KEY_A", "secret-a"),
        ("POOL_KEY_B", "secret-b"),
        ("POOL_KEY_C", "secret-c"),
    ])
}

/// A session id whose pick over `len` entries is `want`.
fn session_for(len: usize, want: usize) -> String {
    (0..)
        .map(|n| format!("session-{n}"))
        .find(|id| pool_index(Some(id.as_str()), len) == want)
        .unwrap()
}

#[test]
fn one_session_keeps_one_key_and_sessions_spread_over_the_pool() {
    let _lock = jcode_base::storage::lock_test_env();
    let _env = pool_env("POOL_KEY_A,POOL_KEY_B,POOL_KEY_C");
    let pool = KeyPool::from_env();
    pool.bind("session-one");
    let pinned = pool.key().unwrap();
    for _ in 0..5 {
        assert_eq!(pool.key().unwrap(), pinned);
    }
    let spread: std::collections::BTreeSet<String> = (0..30)
        .map(|n| {
            pool.bind(&format!("session-{n}"));
            pool.key().unwrap()
        })
        .collect();
    assert_eq!(spread.len(), 3);
}

#[test]
fn fork_keeps_the_pin_and_its_own_binds_stay_separate() {
    let _lock = jcode_base::storage::lock_test_env();
    let _env = pool_env("POOL_KEY_A,POOL_KEY_B,POOL_KEY_C");
    let parent = KeyPool::from_env();
    parent.bind(&session_for(3, 0));
    let fork = parent.fork();
    assert_eq!(fork.key().unwrap(), "secret-a");
    fork.bind(&session_for(3, 1));
    assert_eq!(fork.key().unwrap(), "secret-b");
    assert_eq!(parent.key().unwrap(), "secret-a");
}

#[test]
fn missing_pool_entry_fails_only_its_session_and_names_it() {
    let _lock = jcode_base::storage::lock_test_env();
    let _env = pool_env("POOL_KEY_A,POOL_KEY_MISSING");
    let pool = KeyPool::from_env();
    pool.bind(&session_for(2, 1));
    let err = pool.key().unwrap_err();
    assert!(err.to_string().contains("POOL_KEY_MISSING"), "{err}");
    pool.bind(&session_for(2, 0));
    assert_eq!(pool.key().unwrap(), "secret-a");
}

#[test]
fn unbound_session_uses_the_first_pool_entry() {
    let _lock = jcode_base::storage::lock_test_env();
    let _env = pool_env("POOL_KEY_B,POOL_KEY_A");
    let pool = KeyPool::from_env();
    assert_eq!(pool.key().unwrap(), "secret-b");
}

#[test]
fn unset_first_pool_entry_still_starts_the_pool() {
    let _lock = jcode_base::storage::lock_test_env();
    // POOL_KEY_MISSING is never set. The first entry resolves to nothing.
    let _env = EnvGuard::set(&[
        ("JCODE_ANTHROPIC_API_BASE", "https://gateway.example/v1"),
        ("JCODE_NAMED_PROVIDER_PROFILE", "gateway"),
        (
            "JCODE_ANTHROPIC_API_KEY_NAMES",
            "POOL_KEY_MISSING,POOL_KEY_B,POOL_KEY_C",
        ),
        ("JCODE_ANTHROPIC_API_KEY_NAME", "POOL_KEY_MISSING"),
        ("POOL_KEY_B", "secret-b"),
        ("POOL_KEY_C", "secret-c"),
    ]);
    assert!(jcode_base::provider::anthropic::has_anthropic_api_key());
    let pool = KeyPool::from_env();
    assert_eq!(pool.key().unwrap(), "secret-b");
    pool.bind(&session_for(3, 1));
    assert_eq!(pool.key().unwrap(), "secret-b");
    pool.bind(&session_for(3, 2));
    assert_eq!(pool.key().unwrap(), "secret-c");
    pool.bind(&session_for(3, 0));
    let err = pool.key().unwrap_err();
    assert!(err.to_string().contains("POOL_KEY_MISSING"), "{err}");
}
