use super::*;

/// Isolate context-limit fixtures and restore the original cache on drop.
/// Callers must hold `storage::lock_test_env()` while this guard is alive.
#[must_use]
pub struct ContextLimitCacheGuardForTests {
    saved: HashMap<String, usize>,
}

impl ContextLimitCacheGuardForTests {
    pub fn new() -> Self {
        let mut cache = CONTEXT_LIMIT_CACHE
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Self {
            saved: std::mem::take(&mut *cache),
        }
    }
}

impl Default for ContextLimitCacheGuardForTests {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for ContextLimitCacheGuardForTests {
    fn drop(&mut self) {
        let mut cache = CONTEXT_LIMIT_CACHE
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *cache = std::mem::take(&mut self.saved);
    }
}

/// Clear process-global in-memory catalog services between test fixtures.
pub fn reset_model_catalog_services_for_tests() {
    OPENAI_MODEL_CATALOG_SERVICE.reset_for_tests();
    ANTHROPIC_MODEL_CATALOG_SERVICE.reset_for_tests();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_limit_guard_isolates_and_restores_overrides() {
        let _env = crate::storage::lock_test_env();
        let _original = ContextLimitCacheGuardForTests::new();
        populate_context_limits(HashMap::from([("gpt-5.4".to_string(), 200_000)]));
        {
            let _isolated = ContextLimitCacheGuardForTests::new();
            assert_eq!(cached_context_limit_for_model("gpt-5.4"), None);
            populate_context_limits(HashMap::from([("gpt-5.4".to_string(), 1_050_000)]));
            assert_eq!(cached_context_limit_for_model("gpt-5.4"), Some(1_050_000));
        }
        assert_eq!(cached_context_limit_for_model("gpt-5.4"), Some(200_000));
    }

    #[test]
    fn context_limit_guard_restores_after_unwinding() {
        let _env = crate::storage::lock_test_env();
        let _original = ContextLimitCacheGuardForTests::new();
        populate_context_limits(HashMap::from([("gpt-5.4".to_string(), 200_000)]));
        let result = std::panic::catch_unwind(|| {
            let _isolated = ContextLimitCacheGuardForTests::new();
            populate_context_limits(HashMap::from([("gpt-5.4".to_string(), 1_050_000)]));
            panic!("exercise cache restoration");
        });
        assert!(result.is_err());
        assert_eq!(cached_context_limit_for_model("gpt-5.4"), Some(200_000));
    }
}
