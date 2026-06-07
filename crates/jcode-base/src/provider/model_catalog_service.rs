use std::collections::{HashMap, HashSet};
use std::sync::RwLock;
use std::time::{Duration, Instant, SystemTime};

#[derive(Debug, Clone)]
pub(crate) struct RuntimeModelUnavailability {
    pub(crate) reason: String,
    recorded_at: Instant,
    pub(crate) observed_at: SystemTime,
}

/// Account-scoped live catalog state for one provider family.
///
/// This replaces several parallel globals (available models, fetched time,
/// observed time, last refresh attempt, and in-flight markers) with one explicit
/// service boundary. Persistence still lives in the provider-specific caller so
/// storage paths and schemas remain unchanged during migration.
#[derive(Debug)]
pub(crate) struct ModelCatalogService {
    cache_ttl: Duration,
    retry_interval: Duration,
    runtime_unavailable_ttl: Duration,
    available_models: RwLock<HashMap<String, HashSet<String>>>,
    fetched_at: RwLock<HashMap<String, Instant>>,
    observed_at: RwLock<HashMap<String, SystemTime>>,
    last_attempt: RwLock<HashMap<String, Instant>>,
    in_flight: RwLock<HashSet<String>>,
    runtime_unavailable_models:
        RwLock<HashMap<String, HashMap<String, RuntimeModelUnavailability>>>,
    revision: RwLock<u64>,
}

impl ModelCatalogService {
    pub(crate) fn new(
        cache_ttl: Duration,
        retry_interval: Duration,
        runtime_unavailable_ttl: Duration,
    ) -> Self {
        Self {
            cache_ttl,
            retry_interval,
            runtime_unavailable_ttl,
            available_models: RwLock::new(HashMap::new()),
            fetched_at: RwLock::new(HashMap::new()),
            observed_at: RwLock::new(HashMap::new()),
            last_attempt: RwLock::new(HashMap::new()),
            in_flight: RwLock::new(HashSet::new()),
            runtime_unavailable_models: RwLock::new(HashMap::new()),
            revision: RwLock::new(0),
        }
    }

    pub(crate) fn model_ids(&self, scope: &str) -> Option<Vec<String>> {
        let mut models: Vec<String> = self
            .available_models
            .read()
            .ok()?
            .get(scope)?
            .iter()
            .cloned()
            .collect();
        if models.is_empty() {
            return None;
        }
        models.sort();
        Some(models)
    }

    pub(crate) fn contains_model(&self, scope: &str, model: &str) -> Option<bool> {
        let models = self.available_models.read().ok()?;
        Some(models.get(scope)?.contains(model))
    }

    pub(crate) fn replace_scope_models(
        &self,
        scope: &str,
        models: HashSet<String>,
        observed_at: SystemTime,
    ) -> bool {
        if models.is_empty() {
            return false;
        }

        if let Ok(mut available) = self.available_models.write() {
            available.insert(scope.to_string(), models);
        } else {
            return false;
        }
        if let Ok(mut fetched_at) = self.fetched_at.write() {
            fetched_at.insert(scope.to_string(), Instant::now());
        }
        if let Ok(mut observed_at_map) = self.observed_at.write() {
            observed_at_map.insert(scope.to_string(), observed_at);
        }
        if let Ok(mut revision) = self.revision.write() {
            *revision = revision.wrapping_add(1);
        }
        true
    }

    pub(crate) fn record_runtime_model_unavailable(&self, scope: &str, model: &str, reason: &str) {
        let scope = scope.trim();
        let model = model.trim();
        if scope.is_empty() || model.is_empty() {
            return;
        }
        if let Ok(mut unavailable) = self.runtime_unavailable_models.write() {
            unavailable.entry(scope.to_string()).or_default().insert(
                model.to_string(),
                RuntimeModelUnavailability {
                    reason: reason.trim().to_string(),
                    recorded_at: Instant::now(),
                    observed_at: SystemTime::now(),
                },
            );
        }
    }

    pub(crate) fn runtime_model_unavailability(
        &self,
        scope: &str,
        model: &str,
    ) -> Option<RuntimeModelUnavailability> {
        let mut unavailable = self.runtime_unavailable_models.write().ok()?;
        let models = unavailable.get_mut(scope)?;
        if let Some(entry) = models.get(model) {
            if entry.recorded_at.elapsed() <= self.runtime_unavailable_ttl {
                return Some(entry.clone());
            }
        }
        models.remove(model);
        if models.is_empty() {
            unavailable.remove(scope);
        }
        None
    }

    pub(crate) fn clear_runtime_model_unavailable(&self, scope: &str, model: &str) {
        if let Ok(mut unavailable) = self.runtime_unavailable_models.write() {
            if let Some(models) = unavailable.get_mut(scope) {
                models.remove(model);
                if models.is_empty() {
                    unavailable.remove(scope);
                }
            }
        }
    }

    pub(crate) fn clear_runtime_model_unavailable_scope(&self, scope: &str) {
        if let Ok(mut unavailable) = self.runtime_unavailable_models.write() {
            unavailable.remove(scope);
        }
    }

    pub(crate) fn is_fresh(&self, scope: &str) -> bool {
        self.fetched_at
            .read()
            .ok()
            .and_then(|guard| guard.get(scope).copied())
            .map(|fetched_at| fetched_at.elapsed() <= self.cache_ttl)
            .unwrap_or(false)
    }

    pub(crate) fn observed_at(&self, scope: &str) -> Option<SystemTime> {
        self.observed_at
            .read()
            .ok()
            .and_then(|map| map.get(scope).copied())
    }

    pub(crate) fn note_attempt(&self, scope: &str) {
        if let Ok(mut last_attempt) = self.last_attempt.write() {
            last_attempt.insert(scope.to_string(), Instant::now());
        }
    }

    pub(crate) fn refresh_throttled(&self, scope: &str) -> bool {
        self.last_attempt
            .read()
            .ok()
            .and_then(|last_attempt| last_attempt.get(scope).copied())
            .map(|at| at.elapsed() < self.retry_interval)
            .unwrap_or(false)
    }

    pub(crate) fn should_refresh(&self, scope: &str) -> bool {
        if self.is_fresh(scope) || self.refresh_throttled(scope) {
            return false;
        }
        self.in_flight
            .read()
            .map(|in_flight| !in_flight.contains(scope))
            .unwrap_or(true)
    }

    pub(crate) fn begin_refresh(&self, scope: &str) -> bool {
        if !self.should_refresh(scope) {
            return false;
        }
        let Ok(mut in_flight) = self.in_flight.write() else {
            return false;
        };
        if !in_flight.insert(scope.to_string()) {
            return false;
        }
        self.note_attempt(scope);
        true
    }

    pub(crate) fn finish_refresh(&self, scope: &str) {
        if let Ok(mut in_flight) = self.in_flight.write() {
            in_flight.remove(scope);
        }
    }

    #[cfg(test)]
    pub(crate) fn revision(&self) -> u64 {
        self.revision.read().map(|revision| *revision).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service() -> ModelCatalogService {
        ModelCatalogService::new(
            Duration::from_secs(60),
            Duration::from_secs(60),
            Duration::from_secs(60),
        )
    }

    #[test]
    fn replace_scope_models_updates_catalog_and_revision() {
        let service = service();
        let models = HashSet::from(["gpt-5.5".to_string(), "gpt-5.4".to_string()]);

        assert!(service.replace_scope_models("default", models, SystemTime::now()));

        assert_eq!(
            service.model_ids("default"),
            Some(vec!["gpt-5.4".to_string(), "gpt-5.5".to_string()])
        );
        assert!(service.is_fresh("default"));
        assert_eq!(service.revision(), 1);
    }

    #[test]
    fn begin_refresh_blocks_duplicate_until_finished_then_retry_throttles() {
        let service = service();

        assert!(service.begin_refresh("default"));
        assert!(!service.begin_refresh("default"));

        service.finish_refresh("default");
        assert!(!service.begin_refresh("default"));
    }

    #[test]
    fn empty_model_sets_do_not_replace_existing_catalog() {
        let service = service();
        assert!(!service.replace_scope_models("default", HashSet::new(), SystemTime::now()));
        assert_eq!(service.model_ids("default"), None);
        assert_eq!(service.revision(), 0);
    }

    #[test]
    fn runtime_unavailability_is_account_scoped_and_clearable() {
        let service = service();
        service.record_runtime_model_unavailable("default", "gpt-5.5", "quota exceeded");

        let unavailable = service
            .runtime_model_unavailability("default", "gpt-5.5")
            .expect("runtime marker should be present");
        assert_eq!(unavailable.reason, "quota exceeded");
        assert!(
            service
                .runtime_model_unavailability("other", "gpt-5.5")
                .is_none()
        );

        service.clear_runtime_model_unavailable("default", "gpt-5.5");
        assert!(
            service
                .runtime_model_unavailability("default", "gpt-5.5")
                .is_none()
        );
    }

    #[test]
    fn runtime_unavailability_scope_clear_drops_all_models_for_account() {
        let service = service();
        service.record_runtime_model_unavailable("default", "gpt-5.5", "quota exceeded");
        service.record_runtime_model_unavailable("default", "gpt-5.4", "quota exceeded");

        service.clear_runtime_model_unavailable_scope("default");

        assert!(
            service
                .runtime_model_unavailability("default", "gpt-5.5")
                .is_none()
        );
        assert!(
            service
                .runtime_model_unavailability("default", "gpt-5.4")
                .is_none()
        );
    }
}
